//! A project's evaluation rules: what its evaluators judge — its runs, its
//! spans of a kind and name, or its sessions once quiet — under the trace
//! list's own filters, sampled. Owners and admins make, change and delete
//! one, each filed in the chain and held to the plan's number as alerts are;
//! every member reads them. A rule is previewed against the last seven days
//! before it is turned on: how many targets, and what judging them would cost.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::types::Json as Jsonb;
use uuid::Uuid;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam;
use telmoni_shared::{
    AuditAction, AuthError, ContentMode, EvaluationTarget, ModelProvider, ProjectId, SpanKind,
    TelmoniError, TelmoniResourceKind, ViewKind,
};

use crate::AppState;
use crate::evaluators::{self, VARIABLES};
use crate::handler::chain;
use crate::handler::views::ViewQuery;
use crate::prices::{PriceBook, Usage};
use crate::sessions::Sessions;
use crate::store::{self, Projects};
use crate::traces::{self, ListSort, Traces};
use crate::{contract, db};
use telmoni_shared::{SortDirection, TraceSort};

/// The longest name a rule goes by, the most evaluators one runs, and the
/// longest JSON Pointer it narrows a variable by.
pub const NAME_MAX: usize = 60;
pub const EVALUATORS_MAX: usize = 10;
pub const POINTER_MAX: usize = 200;
/// The days a preview counts over.
pub const PREVIEW_DAYS: u16 = 7;
/// A session judged once quiet this long, unless its project says otherwise;
/// two minutes at least, a day at most.
pub const SESSION_IDLE_MINUTES: u16 = 10;
pub const SESSION_IDLE_RANGE: std::ops::RangeInclusive<u16> = 2..=1440;
/// The newest targets a preview reads the tokens of, for its estimate.
const SAMPLE_TARGETS: u32 = 50;
/// What a preview counts on a judgement's answer: its reasoning and score.
const ANSWER_TOKENS: u64 = 300;

/// A rule as the lanes answer it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EvaluationRule {
    pub id: Uuid,
    pub name: String,
    pub target: EvaluationTarget,
    pub span_kind: Option<SpanKind>,
    pub span_name: Option<String>,
    pub query: Jsonb<ViewQuery>,
    pub sample_rate: f64,
    pub pointers: Jsonb<BTreeMap<String, String>>,
    pub enabled: bool,
    pub backfill_from: Option<DateTime<Utc>>,
    /// How many targets its reach back has looked over, newest first, and
    /// when it finished.
    pub backfill_seen: i32,
    pub backfill_done_at: Option<DateTime<Utc>>,
    /// The evaluators it runs, each by its stable id.
    pub evaluators: Vec<Uuid>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The project's rules, and how long a session is quiet before one judges it.
#[derive(Debug, Serialize)]
pub struct EvaluationRules {
    pub rules: Vec<EvaluationRule>,
    pub session_idle_minutes: u16,
}

/// What a save or a preview sends: the rule's id where it changes one.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleChange {
    #[serde(default)]
    pub id: Option<Uuid>,
    pub name: String,
    pub target: EvaluationTarget,
    #[serde(default)]
    pub span_kind: Option<SpanKind>,
    #[serde(default)]
    pub span_name: Option<String>,
    #[serde(default)]
    pub query: ViewQuery,
    #[serde(default = "every")]
    pub sample_rate: f64,
    #[serde(default)]
    pub pointers: BTreeMap<String, String>,
    pub evaluators: Vec<Uuid>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub backfill_from: Option<DateTime<Utc>>,
}

const fn every() -> f64 {
    1.0
}

const fn yes() -> bool {
    true
}

/// What a preview answers: the last seven days' targets, those the sampling
/// rate would have judged, and what judging them would have cost each
/// evaluator at its model's price, where the price table holds one.
#[derive(Debug, Serialize)]
pub struct Preview {
    pub days: u16,
    pub targets: u64,
    pub judged: u64,
    pub evaluators: Vec<PreviewedEvaluator>,
}

/// One evaluator's share of a preview.
#[derive(Debug, Serialize)]
pub struct PreviewedEvaluator {
    pub id: Uuid,
    pub name: String,
    pub model: String,
    /// In US dollars; none where its model has no price.
    pub cost: Option<f64>,
}

/// The idle time a project's sessions are judged after.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionIdle {
    pub minutes: u16,
}

/// A change held to a rule's rules, each text trimmed and its query to the
/// trace list's own.
fn checked(mut change: RuleChange) -> Result<RuleChange, TelmoniError> {
    let refuse = |why: String| Err(TelmoniError::BadRequest(why));
    change.name = change.name.trim().to_owned();
    if change.name.is_empty() || change.name.chars().count() > NAME_MAX {
        return refuse(format!("a rule's name is 1 to {NAME_MAX} characters"));
    }
    change.span_name = change
        .span_name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty());
    match change.target {
        EvaluationTarget::Span => match change.span_kind {
            None | Some(SpanKind::Run) => {
                return refuse("a span rule names a kind of span other than a run".into());
            }
            Some(_) => {}
        },
        EvaluationTarget::Run | EvaluationTarget::Session => {
            if change.span_kind.is_some() || change.span_name.is_some() {
                return refuse("only a span rule names a kind or a name of span".into());
            }
        }
    }
    if change
        .span_name
        .as_ref()
        .is_some_and(|n| n.chars().count() > contract::NAME_MAX)
    {
        return refuse(format!(
            "a span's name is up to {} characters",
            contract::NAME_MAX
        ));
    }
    if !change.sample_rate.is_finite() || change.sample_rate <= 0.0 || change.sample_rate > 1.0 {
        return refuse("a sampling rate is more than 0 and at most 1".into());
    }
    let query = change.query.checked(ViewKind::Traces)?;
    change.query = ViewQuery {
        sort: None,
        dir: None,
        days: None,
        limit: None,
        ..query
    };
    for (variable, pointer) in &change.pointers {
        if !VARIABLES.contains(&variable.as_str()) {
            return refuse(format!("{{{{{variable}}}}} is no variable a rule narrows"));
        }
        if !(pointer.is_empty() || pointer.starts_with('/')) || pointer.len() > POINTER_MAX {
            return refuse(format!(
                "a variable is narrowed by a JSON Pointer, `/` and a path, up to {POINTER_MAX} characters"
            ));
        }
    }
    change.pointers.retain(|_, pointer| !pointer.is_empty());
    let mut evaluators = change.evaluators.clone();
    evaluators.sort_unstable();
    evaluators.dedup();
    if evaluators.is_empty() || evaluators.len() > EVALUATORS_MAX {
        return refuse(format!("a rule runs 1 to {EVALUATORS_MAX} evaluators"));
    }
    change.evaluators = evaluators;
    if change.backfill_from.is_some_and(|from| from > Utc::now()) {
        return refuse("a rule reaches back to a date already past".into());
    }
    Ok(change)
}

/// Refused where the project keeps no content: nothing for its evaluators to read.
fn content_kept(mode: ContentMode) -> Result<(), TelmoniError> {
    if mode == ContentMode::On {
        Ok(())
    } else {
        Err(TelmoniError::BadRequest(
            "this project keeps no content, so a rule has nothing to judge: switch content on first".into(),
        ))
    }
}

const SELECT: &str =
    "SELECT r.id, r.name, r.target, r.span_kind, r.span_name, r.query, r.sample_rate,
        r.pointers, r.enabled, r.backfill_from, r.backfill_seen, r.backfill_done_at,
        r.created_by, r.created_at, r.updated_at,
        COALESCE(array_agg(e.evaluator_id ORDER BY e.evaluator_id)
                 FILTER (WHERE e.evaluator_id IS NOT NULL), '{}') AS evaluators
   FROM telemetry.evaluation_rules r
   LEFT JOIN telemetry.evaluation_rule_evaluators e ON e.rule_id = r.id";

/// `GET /internal/v1/telemetry/evaluation-rules` — the project's rules and
/// its sessions' idle time, every member's to read.
pub async fn get_rules(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<EvaluationRules>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Project)?
        .project_id
        .clone();
    let mut tx = project_scope(&state.db, &project_id).await?;
    let rules = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE r.project_id = $1 GROUP BY r.id ORDER BY r.name"
    )))
    .bind(&project_id)
    .fetch_all(tx.conn())
    .await?;
    let idle = session_idle(&mut tx, &project_id).await?;
    tx.commit().await?;
    Ok(Json(EvaluationRules {
        rules,
        session_idle_minutes: idle,
    }))
}

/// The project's sessions' idle time, in minutes.
///
/// # Errors
///
/// Postgres did not answer.
pub async fn session_idle<B: telmoni_shared::db::tenant_session::HasProject>(
    tx: &mut telmoni_shared::db::tenant_session::Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<u16> {
    let minutes: Option<Option<i16>> = sqlx::query_scalar(
        "SELECT session_idle_minutes FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?;
    Ok(minutes
        .flatten()
        .and_then(|m| u16::try_from(m).ok())
        .unwrap_or(SESSION_IDLE_MINUTES))
}

/// `PUT /internal/v1/telemetry/evaluation-rules` — a rule made, or changed
/// by its id, by an owner or admin: its evaluators the project's own, a new
/// one held to the plan's number, then written and filed.
pub async fn put_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(change): Json<RuleChange>,
) -> Result<Json<EvaluationRule>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Project)?
        .project_id
        .clone();
    let change = checked(change)?;
    if let Some(from) = change.backfill_from {
        let reach = crate::capacity::visible_days(&state, &project_id, traces::MAX_DAYS).await;
        if from < Utc::now() - Duration::days(i64::from(reach)) {
            return Err(TelmoniError::BadRequest(format!(
                "a rule reaches back no further than the {reach} days this project keeps"
            )));
        }
    }
    let ceilings = seam::alert_ceilings(state.entitlements.as_ref(), &acting.organization_id).await;
    let id = change.id.unwrap_or_else(Uuid::new_v4);
    let fresh = change.id.is_none();
    let auth = &*state.auth;
    let written = chain::write(&state.db, &acting, &project_id, "an evaluation rule", async |tx| {
        chain::held(auth, &acting, tx, Verb::Update, Resource::Project).await?;
        content_kept(db::content_mode(tx, &project_id).await?)?;
        let known: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT id) FROM telemetry.evaluators WHERE project_id = $1 AND id = ANY($2)",
        )
        .bind(&project_id)
        .bind(&change.evaluators)
        .fetch_one(tx.conn())
        .await?;
        if usize::try_from(known).unwrap_or(0) != change.evaluators.len() {
            return Err(TelmoniError::BadRequest("a rule runs this project's own evaluators".into()));
        }
        if fresh
            && let Some(ceilings) = &ceilings
        {
                let held: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM telemetry.evaluation_rules WHERE organization_id = $1",
                )
                .bind(&acting.organization_id)
                .fetch_one(tx.conn())
                .await?;
                ceilings.admit_evaluation_rule(u64::try_from(held).unwrap_or(0))?;
            }
        let touched = if fresh {
            sqlx::query(
                "INSERT INTO telemetry.evaluation_rules
                     (id, project_id, organization_id, name, target, span_kind, span_name, query,
                      sample_rate, pointers, enabled, backfill_from, created_by)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
            )
            .bind(id)
            .bind(&project_id)
            .bind(&acting.organization_id)
            .bind(&change.name)
            .bind(change.target)
            .bind(change.span_kind)
            .bind(&change.span_name)
            .bind(Jsonb(&change.query))
            .bind(change.sample_rate)
            .bind(Jsonb(&change.pointers))
            .bind(change.enabled)
            .bind(change.backfill_from)
            .bind(acting.user_id.as_str())
            .execute(tx.conn())
            .await?
        } else {
            sqlx::query(
                "UPDATE telemetry.evaluation_rules
                    SET name = $3, target = $4, span_kind = $5, span_name = $6, query = $7,
                        sample_rate = $8, pointers = $9, enabled = $10,
                        backfill_seen = CASE WHEN backfill_from IS DISTINCT FROM $11 THEN 0 ELSE backfill_seen END,
                        backfill_done_at = CASE WHEN backfill_from IS DISTINCT FROM $11 THEN NULL ELSE backfill_done_at END,
                        backfill_from = $11, updated_at = now()
                  WHERE id = $1 AND project_id = $2",
            )
            .bind(id)
            .bind(&project_id)
            .bind(&change.name)
            .bind(change.target)
            .bind(change.span_kind)
            .bind(&change.span_name)
            .bind(Jsonb(&change.query))
            .bind(change.sample_rate)
            .bind(Jsonb(&change.pointers))
            .bind(change.enabled)
            .bind(change.backfill_from)
            .execute(tx.conn())
            .await?
        }
        .rows_affected();
        if touched == 0 {
            return Err(AuthError::NotFound("no evaluation rule with that id in this project".into()).into());
        }
        sqlx::query("DELETE FROM telemetry.evaluation_rule_evaluators WHERE rule_id = $1")
            .bind(id)
            .execute(tx.conn())
            .await?;
        sqlx::query(
            "INSERT INTO telemetry.evaluation_rule_evaluators (rule_id, evaluator_id, project_id)
             SELECT $1, unnest($2::uuid[]), $3",
        )
        .bind(id)
        .bind(&change.evaluators)
        .bind(&project_id)
        .execute(tx.conn())
        .await?;
        let row: EvaluationRule = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{SELECT} WHERE r.id = $1 GROUP BY r.id"
        )))
        .bind(id)
        .fetch_one(tx.conn())
        .await?;
        let details = json!({
            "target": row.target, "evaluators": row.evaluators.len(),
            "sample_rate": row.sample_rate, "enabled": row.enabled,
        });
        if fresh {
            emit_audit(
                tx,
                AuditEvent::new(
                    &acting.organization_id,
                    Actor::User(acting.user_id.as_str()),
                    AuditAction::Created,
                    TelmoniResourceKind::EvaluationRule,
                )
                .in_project(&project_id)
                .on(&id.to_string())
                .details(details),
            )
            .await?;
        } else {
            emit_audit(
                tx,
                AuditEvent::new(
                    &acting.organization_id,
                    Actor::User(acting.user_id.as_str()),
                    AuditAction::Updated,
                    TelmoniResourceKind::EvaluationRule,
                )
                .in_project(&project_id)
                .on(&id.to_string())
                .details(details),
            )
            .await?;
        }
        Ok(row)
    })
    .await
    .map_err(|e| match e {
        TelmoniError::Database(sqlx::Error::Database(db)) if db.constraint() == Some("evaluation_rules_name_key") => {
            AuthError::Conflict("this project has an evaluation rule by that name already".into()).into()
        }
        other => other,
    })?;
    tracing::info!(project_id = %project_id, rule_id = %id, "telemetry: an evaluation rule saved");
    Ok(Json(written))
}

/// `DELETE /internal/v1/telemetry/evaluation-rules/{id}` — a rule gone, by
/// an owner or admin, filed in the chain; its evaluators stay.
pub async fn delete_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Project)?
        .project_id
        .clone();
    let auth = &*state.auth;
    chain::write(
        &state.db,
        &acting,
        &project_id,
        "an evaluation rule",
        async |tx| {
            chain::held(auth, &acting, tx, Verb::Update, Resource::Project).await?;
            let gone = sqlx::query(
                "DELETE FROM telemetry.evaluation_rules WHERE project_id = $1 AND id = $2",
            )
            .bind(&project_id)
            .bind(id)
            .execute(tx.conn())
            .await?
            .rows_affected();
            if gone == 0 {
                return Err(AuthError::NotFound(
                    "no evaluation rule with that id in this project".into(),
                )
                .into());
            }
            emit_audit(
                tx,
                AuditEvent::new(
                    &acting.organization_id,
                    Actor::User(acting.user_id.as_str()),
                    AuditAction::Deleted,
                    TelmoniResourceKind::EvaluationRule,
                )
                .in_project(&project_id)
                .on(&id.to_string()),
            )
            .await?;
            Ok(())
        },
    )
    .await?;
    tracing::info!(project_id = %project_id, rule_id = %id, "telemetry: an evaluation rule deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /internal/v1/telemetry/evaluation-rules/session-idle` — how long a
/// session is quiet before a rule judges it, by an owner or admin.
pub async fn put_session_idle(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(idle): Json<SessionIdle>,
) -> Result<StatusCode, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Project)?
        .project_id
        .clone();
    if !SESSION_IDLE_RANGE.contains(&idle.minutes) {
        return Err(TelmoniError::BadRequest(
            "a session is judged after 2 to 1,440 quiet minutes".into(),
        ));
    }
    let auth = &*state.auth;
    chain::write(&state.db, &acting, &project_id, "a session's idle time", async |tx| {
        chain::held(auth, &acting, tx, Verb::Update, Resource::Project).await?;
        db::ensure_settings(tx, &project_id, &acting.organization_id).await?;
        sqlx::query("UPDATE telemetry.project_settings SET session_idle_minutes = $2 WHERE project_id = $1")
            .bind(&project_id)
            .bind(i16::try_from(idle.minutes).unwrap_or(i16::MAX))
            .execute(tx.conn())
            .await?;
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The ClickHouse provider name the price table knows a connection's
/// provider by; none for an endpoint of the customer's own.
#[must_use]
pub const fn priced_as(provider: ModelProvider) -> Option<&'static str> {
    match provider {
        ModelProvider::Openai => Some("openai"),
        ModelProvider::Anthropic => Some("anthropic"),
        ModelProvider::GoogleAiStudio => Some("gcp.gemini"),
        ModelProvider::VertexAi => Some("gcp.vertex_ai"),
        ModelProvider::AzureOpenai => Some("azure.ai.openai"),
        ModelProvider::Bedrock => Some("aws.bedrock"),
        ModelProvider::Typesafe => Some("typesafe"),
        ModelProvider::OpenaiCompatible | ModelProvider::SystemOneCompatible => None,
    }
}

/// `POST /internal/v1/telemetry/evaluation-rules/preview` — a draft rule
/// against the last seven days: the targets it would have found, those its
/// sampling would have judged, and each evaluator's cost at its model's price.
pub async fn preview_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(change): Json<RuleChange>,
) -> Result<Json<Preview>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Project)?
        .project_id
        .clone();
    let change = checked(change)?;
    let projects = Projects::one(project_id.clone());
    let filter = change.query.filter(PREVIEW_DAYS)?;
    let failed = |e: clickhouse::error::Error| {
        store::failed(
            &e,
            "an evaluation rule's preview",
            "the span store did not answer; try again",
        )
    };
    let (targets, tokens) = match change.target {
        EvaluationTarget::Run => {
            let targets = traces::count::<Traces>(&state.store, &projects, &filter)
                .await
                .map_err(failed)?;
            let newest = traces::list::<Traces>(
                &state.store,
                &projects,
                &filter,
                (&ListSort::Column(TraceSort::Started), SortDirection::Desc),
                SAMPLE_TARGETS,
                0,
            )
            .await
            .map_err(failed)?;
            (
                targets,
                mean(newest.iter().map(|r| r.input_tokens + r.output_tokens)),
            )
        }
        EvaluationTarget::Session => {
            let targets = traces::count::<Sessions>(&state.store, &projects, &filter)
                .await
                .map_err(failed)?;
            let newest = traces::list::<Sessions>(
                &state.store,
                &projects,
                &filter,
                (
                    &ListSort::Column(telmoni_shared::SessionSort::LastSeen),
                    SortDirection::Desc,
                ),
                SAMPLE_TARGETS,
                0,
            )
            .await
            .map_err(failed)?;
            (
                targets,
                mean(newest.iter().map(|s| s.input_tokens + s.output_tokens)),
            )
        }
        EvaluationTarget::Span => {
            // As the loop finds them: spans of the kind and name in the runs
            // the rule's filters admit.
            let counted = traces::spans_in_runs(
                &state.store,
                &projects,
                &filter,
                &change.span_kind.map(|k| k.to_string()).unwrap_or_default(),
                change.span_name.as_deref().unwrap_or_default(),
            )
            .await
            .map_err(failed)?;
            (counted.total, counted.tokens)
        }
    };
    let judged = sampled_count(targets, change.sample_rate);
    let evaluators = priced(&state, &project_id, &change.evaluators, tokens, judged).await?;
    Ok(Json(Preview {
        days: PREVIEW_DAYS,
        targets,
        judged,
        evaluators,
    }))
}

/// The mean of `each`, none for none.
fn mean(each: impl Iterator<Item = u64>) -> u64 {
    let (sum, n) = each.fold((0_u64, 0_u64), |(s, n), t| (s.saturating_add(t), n + 1));
    sum.checked_div(n).unwrap_or(0)
}

/// How many of `targets` a rate of `rate` judges, rounded.
fn sampled_count(targets: u64, rate: f64) -> u64 {
    if rate >= 1.0 {
        return targets;
    }
    // A rate in thousandths: the preview's estimate needs no finer.
    let thousandths = (1..=1000_u64)
        .find(|t| f64::from(u32::try_from(*t).unwrap_or(1000)) / 1000.0 >= rate)
        .unwrap_or(1000);
    (targets.saturating_mul(thousandths) + 500) / 1000
}

/// Each evaluator's newest version priced for `judged` judgements, each
/// sending its prompt and `tokens` of what it judges, and answering
/// [`ANSWER_TOKENS`].
async fn priced(
    state: &AppState,
    project_id: &ProjectId,
    ids: &[Uuid],
    tokens: u64,
    judged: u64,
) -> Result<Vec<PreviewedEvaluator>, TelmoniError> {
    #[derive(sqlx::FromRow)]
    struct Judge {
        id: Uuid,
        name: String,
        messages: Option<Jsonb<Vec<evaluators::Message>>>,
        state: Option<String>,
        questions: Option<Jsonb<Vec<telmoni_shared::model::systemone::Question>>>,
        model: Option<String>,
        provider: Option<ModelProvider>,
        models: Option<Vec<String>>,
    }
    let mut tx = project_scope(&state.db, project_id).await?;
    let judges: Vec<Judge> = sqlx::query_as(
        "SELECT DISTINCT ON (e.id) e.id, e.name, e.messages, e.state, e.questions, e.model,
                c.provider, c.models
           FROM telemetry.evaluators e
           LEFT JOIN telemetry.model_connections c
             ON c.project_id = e.project_id
            AND (c.id = e.connection_id OR (e.connection_id IS NULL AND c.is_default))
          WHERE e.project_id = $1 AND e.id = ANY($2)
          ORDER BY e.id, e.version DESC",
    )
    .bind(project_id)
    .bind(ids)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    let names: Vec<(String, String)> = judges
        .iter()
        .filter_map(|j| {
            let model = j
                .model
                .clone()
                .or_else(|| j.models.as_ref()?.first().cloned())?;
            Some((priced_as(j.provider?)?.to_owned(), model))
        })
        .collect();
    let book = PriceBook::read_for(&state.db, std::slice::from_ref(project_id), &names).await?;
    Ok(judges
        .into_iter()
        .map(|judge| {
            let model = judge
                .model
                .clone()
                .or_else(|| judge.models.as_ref().and_then(|m| m.first().cloned()))
                .unwrap_or_default();
            // A typed one's questions are sent with its state, as a prompt's messages are.
            let prompt: usize = judge
                .messages
                .iter()
                .flat_map(|m| m.iter())
                .map(|m| m.content.len())
                .sum::<usize>()
                + judge.state.as_ref().map_or(0, String::len)
                + judge.questions.as_ref().map_or(0, |q| {
                    serde_json::to_string(&q.0).map_or(0, |text| text.len())
                });
            let usage = Usage {
                input_tokens: (u64::try_from(prompt).unwrap_or(0) / 4 + tokens)
                    .saturating_mul(judged),
                // A typed one writes nothing.
                output_tokens: if judge.state.is_some() {
                    0
                } else {
                    ANSWER_TOKENS
                }
                .saturating_mul(judged),
                ..Usage::default()
            };
            let cost = judge
                .provider
                .and_then(priced_as)
                .and_then(|provider| book.price(provider, &model, Utc::now()))
                .map(|price| price.cost(usage));
            PreviewedEvaluator {
                id: judge.id,
                name: judge.name,
                model,
                cost,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change() -> RuleChange {
        RuleChange {
            id: None,
            name: " Every run ".into(),
            target: EvaluationTarget::Run,
            span_kind: None,
            span_name: None,
            query: ViewQuery {
                agent: vec![" support ".into()],
                sort: Some("agent".into()),
                ..ViewQuery::default()
            },
            sample_rate: 0.5,
            pointers: BTreeMap::from([
                ("input".into(), "/0/content".into()),
                ("output".into(), String::new()),
            ]),
            evaluators: vec![Uuid::nil(), Uuid::nil()],
            enabled: true,
            backfill_from: None,
        }
    }

    /// A rule keeps the list's filters without its sort, a span kind only
    /// where it judges spans, a rate within (0, 1], and its pointers' variables.
    #[test]
    fn a_rule_is_held_to_its_rules() {
        let kept = checked(change()).expect("kept");
        assert_eq!(kept.name, "Every run");
        assert_eq!(kept.query.agent, ["support"]);
        assert_eq!(kept.query.sort, None);
        assert_eq!(kept.evaluators.len(), 1);
        assert_eq!(kept.pointers.len(), 1, "an empty pointer narrows nothing");
        assert!(
            checked(RuleChange {
                sample_rate: 0.0,
                ..change()
            })
            .is_err()
        );
        assert!(
            checked(RuleChange {
                span_kind: Some(SpanKind::ToolCall),
                ..change()
            })
            .is_err()
        );
        assert!(
            checked(RuleChange {
                target: EvaluationTarget::Span,
                ..change()
            })
            .is_err()
        );
        assert!(
            checked(RuleChange {
                target: EvaluationTarget::Span,
                span_kind: Some(SpanKind::ToolCall),
                ..change()
            })
            .is_ok()
        );
        assert!(
            checked(RuleChange {
                pointers: BTreeMap::from([("answer".into(), "/a".into())]),
                ..change()
            })
            .is_err()
        );
        assert!(
            checked(RuleChange {
                evaluators: vec![],
                ..change()
            })
            .is_err()
        );
    }

    /// A rate judges its share of the targets, rounded.
    #[test]
    fn a_rate_judges_its_share() {
        assert_eq!(sampled_count(10, 1.0), 10);
        assert_eq!(sampled_count(10, 0.25), 3);
        assert_eq!(mean([].into_iter()), 0);
        assert_eq!(mean([2, 4].into_iter()), 3);
    }
}
