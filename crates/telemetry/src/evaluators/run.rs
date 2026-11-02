//! The evaluation loop. Each minute one replica finds every running rule's
//! new targets with the trace list's own reads, samples them by a hash of
//! the target's key so a replay samples the same, and writes each once per
//! evaluator to `telemetry.evaluations`. Every replica then leases due rows,
//! five an organization at a time, `FOR UPDATE SKIP LOCKED`, and settles
//! each in its own transaction: judged, its score written and its own trace
//! sent through ingest's path; delayed by a rate limit or a weekly limit;
//! paused while its connection is refused or its project keeps no content;
//! or failed, with its reason.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Datelike as _, Days, SubsecRound as _, TimeZone as _, Utc};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use sqlx::types::Json as Jsonb;
use uuid::Uuid;

use telmoni_shared::acting::ActingKey;
use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::model::{self, Ask, Refused};
use telmoni_shared::seam::{self, OrganizationAlert};
use telmoni_shared::{
    ContentMode, EvaluationTarget, ModelProvider, OrganizationId, ProjectId, ScoreSource,
    SortDirection, SpanKind, TelmoniError, TraceSort,
};

use crate::db::{self, TelemetryLane};
use crate::evaluators::{self, Message, Prompt, ScoreSpec};
use crate::handler::evaluation_rules::{priced_as, session_idle};
use crate::handler::views::ViewQuery;
use crate::ingest::decode::Encoding;
use crate::prices::{PriceBook, Usage};
use crate::scores::{ScoreRow, TargetKey};
use crate::sessions::Sessions;
use crate::store::{Projects, SpanRow};
use crate::traces::{self, ListSort, Traces};
use crate::{AppState, content, contract};

/// The loop's name on the watchdog's heartbeat.
pub const HEARTBEAT: &str = "evaluations";
/// How often it runs.
pub const TICK: Duration = Duration::from_secs(60);
/// The most rows an organization has leased at once.
pub const PER_ORGANIZATION: i64 = 5;
/// The most rows a pass leases at once.
const BATCH: i64 = 50;
/// How long a lease holds before another replica may take the row.
const LEASE_SECS: i64 = 180;
/// The newest targets a rule is looked over for each minute: past them a
/// burst is sampled down to what the minute held.
const NEWEST: u32 = 100;
/// The targets a rule's reach back looks over each minute, newest first.
const BACKFILL_PAGE: u32 = 500;
/// How long after a run's own span arrives it is judged, so its last
/// children have landed.
const SETTLE_SECS: i64 = 60;
/// The tries a provider's failure gets before the evaluation is failed.
const ATTEMPTS_MAX: i16 = 5;
/// How long a paused evaluation waits before it is looked at again.
const PAUSED_SECS: i64 = 300;
/// The most runs of a session read for its judgement, and the most spans of a run.
const SESSION_RUNS_MAX: u32 = 50;
const SPANS_MAX: u32 = 500;
/// The most an answer may run to.
const ANSWER_TOKENS_MAX: u32 = 1_024;

/// [`pass`], every [`TICK`] until the replica stops.
pub async fn run(state: Arc<AppState>) {
    state
        .heartbeats
        .every(&state.db, HEARTBEAT, Duration::ZERO, TICK, || {
            let state = state.clone();
            async move { pass(&state).await.map(drop) }
        })
        .await;
}

/// New targets found, where this replica wins the minute's lock, then every
/// due evaluation judged: answers how many were settled.
///
/// # Errors
///
/// Postgres did not answer.
pub async fn pass(state: &Arc<AppState>) -> Result<u64, TelmoniError> {
    discover(state).await?;
    let mut settled = 0;
    loop {
        let leased = lease(state).await?;
        let count = leased.len();
        for evaluation in leased {
            if settle(state, evaluation).await? {
                settled += 1;
            }
        }
        if i64::try_from(count).unwrap_or(i64::MAX) < BATCH {
            return Ok(settled);
        }
    }
}

/// A rule as the loop reads it, its evaluators' newest versions with it.
#[derive(sqlx::FromRow)]
struct RuleRow {
    id: Uuid,
    project_id: ProjectId,
    organization_id: OrganizationId,
    target: EvaluationTarget,
    span_kind: Option<SpanKind>,
    span_name: Option<String>,
    query: Jsonb<ViewQuery>,
    sample_rate: f64,
    backfill_from: Option<DateTime<Utc>>,
    backfill_seen: i32,
    backfill_done_at: Option<DateTime<Utc>>,
    evaluators: Vec<Uuid>,
    versions: Vec<i32>,
}

/// Which of a rule's targets a read looks over: the newest of the last
/// `days`, past the first `offset`, and none from before `since`.
#[derive(Debug, Clone, Copy)]
struct Reach {
    days: u16,
    limit: u32,
    offset: u64,
    since: Option<DateTime<Utc>>,
}

/// One target found: its key, and the ids the evaluation row names.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub key: String,
    pub run_id: Option<Uuid>,
    pub span_id: Option<String>,
    pub session_id: Option<String>,
}

/// Whether `key` is among the `rate` share of targets a rule judges: by the
/// first eight bytes of its SHA-256, so the same target is always sampled alike.
#[must_use]
pub fn sampled(key: &str, rate: f64) -> bool {
    if rate >= 1.0 {
        return true;
    }
    let digest = Sha256::digest(key.as_bytes());
    let mut first = [0_u8; 8];
    first.copy_from_slice(digest.get(..8).unwrap_or(&[0; 8]));
    let drawn = u64::from_be_bytes(first) >> 11;
    // 53 bits, which an f64 holds whole.
    let unit =
        f64::from(u32::try_from(drawn >> 21).unwrap_or(u32::MAX)) / f64::from(1_u32 << 31) / 2.0;
    unit < rate
}

/// Every running rule of a project that keeps content, its new targets
/// written once per evaluator: on the replica that wins the minute's lock.
async fn discover(state: &Arc<AppState>) -> Result<(), TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let won: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext('telemetry:evaluations'))")
            .fetch_one(tx.conn())
            .await?;
    if !won {
        tx.commit().await?;
        return Ok(());
    }
    let rules: Vec<RuleRow> = sqlx::query_as(
        "SELECT r.id, r.project_id, r.organization_id, r.target, r.span_kind, r.span_name, r.query,
                r.sample_rate, r.backfill_from, r.backfill_seen, r.backfill_done_at,
                array_agg(v.id ORDER BY v.id) AS evaluators,
                array_agg(v.version ORDER BY v.id) AS versions
           FROM telemetry.evaluation_rules r
           JOIN telemetry.project_settings s ON s.project_id = r.project_id AND s.content_mode = 'on'
           JOIN telemetry.evaluation_rule_evaluators l ON l.rule_id = r.id
           JOIN LATERAL (SELECT e.id, e.version FROM telemetry.evaluators e
                          WHERE e.project_id = r.project_id AND e.id = l.evaluator_id
                          ORDER BY e.version DESC LIMIT 1) v ON true
          WHERE r.enabled
          GROUP BY r.id",
    )
    .fetch_all(tx.conn())
    .await?;
    for rule in &rules {
        let newest = Reach {
            days: 1,
            limit: NEWEST,
            offset: 0,
            since: None,
        };
        let mut targets = match targets_of(state, rule, newest).await {
            Ok((targets, _)) => targets,
            Err(error) => {
                tracing::warn!(rule_id = %rule.id, project_id = %rule.project_id, error = %error,
                    "telemetry: an evaluation rule's targets could not be read; looked for again next minute");
                continue;
            }
        };
        if let (Some(from), None) = (rule.backfill_from, rule.backfill_done_at) {
            let back = Reach {
                days: u16::try_from((Utc::now() - from).num_days() + 1)
                    .unwrap_or(traces::MAX_DAYS)
                    .clamp(1, traces::MAX_DAYS),
                limit: BACKFILL_PAGE,
                offset: u64::try_from(rule.backfill_seen).unwrap_or(0),
                since: Some(from),
            };
            match targets_of(state, rule, back).await {
                Ok((found, listed)) => {
                    targets.extend(found);
                    sqlx::query(
                        "UPDATE telemetry.evaluation_rules
                            SET backfill_seen = backfill_seen + $2,
                                backfill_done_at = CASE WHEN $3 THEN now() END
                          WHERE id = $1",
                    )
                    .bind(rule.id)
                    .bind(i32::try_from(listed).unwrap_or(i32::MAX))
                    .bind(listed < u64::from(BACKFILL_PAGE))
                    .execute(tx.conn())
                    .await?;
                }
                Err(error) => {
                    tracing::warn!(rule_id = %rule.id, project_id = %rule.project_id, error = %error,
                        "telemetry: an evaluation rule's reach back could not be read; tried again next minute");
                }
            }
        }
        for target in targets.iter().filter(|t| sampled(&t.key, rule.sample_rate)) {
            for (evaluator, version) in rule.evaluators.iter().zip(&rule.versions) {
                sqlx::query(
                    "INSERT INTO telemetry.evaluations
                         (id, project_id, organization_id, rule_id, evaluator_id, evaluator_version,
                          target, target_key, run_id, span_id, session_id)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                     ON CONFLICT (project_id, evaluator_id, target_key) DO NOTHING",
                )
                .bind(Uuid::new_v4())
                .bind(&rule.project_id)
                .bind(&rule.organization_id)
                .bind(rule.id)
                .bind(evaluator)
                .bind(version)
                .bind(rule.target)
                .bind(&target.key)
                .bind(target.run_id)
                .bind(&target.span_id)
                .bind(&target.session_id)
                .execute(tx.conn())
                .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(())
}

/// A rule's targets within `reach` the list's own reads find, each ready —
/// a run whose own span landed a minute ago, its spans of the rule's kind
/// and name, or a session quiet for the project's idle time — and how many
/// rows the read listed, which moves a reach back.
async fn targets_of(
    state: &AppState,
    rule: &RuleRow,
    reach: Reach,
) -> Result<(Vec<Target>, u64), TelmoniError> {
    let projects = Projects::one(rule.project_id.clone());
    let filter = rule.query.filter(reach.days)?;
    let failed = |e: clickhouse::error::Error| {
        crate::store::failed(
            &e,
            "an evaluation rule's targets",
            "looked for again next minute",
        )
    };
    let now = Utc::now();
    let since = |at: DateTime<Utc>| reach.since.is_none_or(|since| at >= since);
    let count = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
    if rule.target == EvaluationTarget::Session {
        let mut tx = project_scope(&state.db, &rule.project_id).await?;
        let idle = session_idle(&mut tx, &rule.project_id).await?;
        tx.commit().await?;
        let rows = traces::list::<Sessions>(
            &state.store,
            &projects,
            &filter,
            (
                &ListSort::Column(telmoni_shared::SessionSort::LastSeen),
                SortDirection::Desc,
            ),
            reach.limit,
            reach.offset,
        )
        .await
        .map_err(failed)?;
        let listed = count(rows.len());
        let targets = rows
            .into_iter()
            .filter(|s| (now - s.last_seen).num_minutes() >= i64::from(idle) && since(s.last_seen))
            .map(|s| Target {
                key: format!("session/{}", s.session_id),
                run_id: None,
                span_id: None,
                session_id: Some(s.session_id),
            })
            .collect();
        return Ok((targets, listed));
    }
    let rows = traces::list::<Traces>(
        &state.store,
        &projects,
        &filter,
        (&ListSort::Column(TraceSort::Started), SortDirection::Desc),
        reach.limit,
        reach.offset,
    )
    .await
    .map_err(failed)?;
    let listed = count(rows.len());
    let runs: Vec<Uuid> = rows
        .into_iter()
        .filter(|r| {
            r.ended && (now - r.ended_at).num_seconds() >= SETTLE_SECS && since(r.started_at)
        })
        .map(|r| r.run_id)
        .collect();
    if rule.target == EvaluationTarget::Run {
        let targets = runs
            .into_iter()
            .map(|run| Target {
                key: format!("run/{run}"),
                run_id: Some(run),
                span_id: None,
                session_id: None,
            })
            .collect();
        return Ok((targets, listed));
    }
    if runs.is_empty() {
        return Ok((Vec::new(), listed));
    }
    let spans: Vec<SpanOf> = state
        .store
        .read(&projects, SPANS_OF_SQL)
        .param("runs", &runs)
        .param(
            "kind",
            rule.span_kind.map(|k| k.to_string()).unwrap_or_default(),
        )
        .param("name", rule.span_name.clone().unwrap_or_default())
        .fetch_all()
        .await
        .map_err(failed)?;
    let targets = spans
        .into_iter()
        .map(|s| Target {
            key: format!("span/{}/{}", s.run_id, s.span_id),
            run_id: Some(s.run_id),
            span_id: Some(s.span_id),
            session_id: None,
        })
        .collect();
    Ok((targets, listed))
}

/// A rule's spans of its kind and name, among the runs found.
const SPANS_OF_SQL: &str = "SELECT DISTINCT run_id, span_id FROM telemetry.spans \
     WHERE project_id IN {projects:Array(String)} AND run_id IN {runs:Array(UUID)} \
       AND kind = {kind:String} AND ({name:String} = '' OR name = {name:String}) \
     LIMIT 1000";

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct SpanOf {
    #[serde(with = "clickhouse::serde::uuid")]
    run_id: Uuid,
    span_id: String,
}

/// One leased evaluation.
#[derive(Debug, sqlx::FromRow)]
struct Leased {
    id: Uuid,
    project_id: ProjectId,
    organization_id: OrganizationId,
    evaluator_id: Uuid,
    evaluator_version: i32,
    rule_id: Option<Uuid>,
    target: EvaluationTarget,
    run_id: Option<Uuid>,
    span_id: Option<String>,
    session_id: Option<String>,
    attempts: i16,
    lease_id: Option<Uuid>,
}

/// Up to [`BATCH`] due evaluations, [`PER_ORGANIZATION`] an organization,
/// leased for [`LEASE_SECS`] so no other replica takes them meanwhile.
async fn lease(state: &AppState) -> Result<Vec<Leased>, TelmoniError> {
    let lease_id = Uuid::new_v4();
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let leased: Vec<Leased> = sqlx::query_as(
        "WITH due AS (
             SELECT id, organization_id,
                    row_number() OVER (PARTITION BY organization_id ORDER BY due_at) AS nth
               FROM telemetry.evaluations
              WHERE status IN ('pending', 'delayed') AND due_at <= now()
         ), taken AS (
             SELECT e.id FROM telemetry.evaluations e JOIN due USING (id)
              WHERE due.nth <= $1
              ORDER BY e.due_at LIMIT $2
                FOR UPDATE OF e SKIP LOCKED
         )
         UPDATE telemetry.evaluations e
            SET lease_id = $3, due_at = now() + make_interval(secs => $4::double precision)
           FROM taken WHERE e.id = taken.id
         RETURNING e.id, e.project_id, e.organization_id, e.evaluator_id, e.evaluator_version,
                   e.rule_id, e.target, e.run_id, e.span_id, e.session_id, e.attempts, e.lease_id",
    )
    .bind(PER_ORGANIZATION)
    .bind(BATCH)
    .bind(lease_id)
    .bind(LEASE_SECS)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(leased)
}

/// How one evaluation ended this pass.
enum Outcome {
    /// Judged: its score, its trace, what it used.
    Done {
        score_id: Uuid,
        trace_run_id: Option<Uuid>,
        provider: String,
        model: String,
        input_tokens: u64,
        output_tokens: u64,
        cost: Option<f64>,
        cut: bool,
        read_at: DateTime<Utc>,
    },
    /// Looked at again at `until`, its reason kept.
    Wait {
        until: DateTime<Utc>,
        status: &'static str,
        reason: String,
        tried: bool,
    },
    /// Given up.
    Failed {
        reason: String,
        read_at: Option<DateTime<Utc>>,
    },
}

/// One leased evaluation worked and settled under its lease: answers whether
/// it was done or failed.
async fn settle(state: &Arc<AppState>, leased: Leased) -> Result<bool, TelmoniError> {
    let outcome = match judge(state, &leased).await {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::warn!(evaluation_id = %leased.id, project_id = %leased.project_id, error = %error,
                "telemetry: an evaluation could not be worked; tried again shortly");
            Outcome::Wait {
                until: backoff(leased.attempts),
                status: "pending",
                reason: "it could not be worked; tried again shortly".into(),
                tried: true,
            }
        }
    };
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let settled = match outcome {
        Outcome::Done {
            score_id,
            trace_run_id,
            provider,
            model,
            input_tokens,
            output_tokens,
            cost,
            cut,
            read_at,
        } => {
            sqlx::query(
                "UPDATE telemetry.evaluations
                    SET status = 'done', reason = NULL, score_id = $3, trace_run_id = $4,
                        provider = $5, model = $6, input_tokens = $7, output_tokens = $8,
                        cost = $9, cut = $10, read_at = $11, attempts = attempts + 1,
                        settled_at = now(), lease_id = NULL
                  WHERE id = $1 AND lease_id = $2",
            )
            .bind(leased.id)
            .bind(leased.lease_id)
            .bind(score_id)
            .bind(trace_run_id)
            .bind(provider)
            .bind(model)
            .bind(i64::try_from(input_tokens).unwrap_or(i64::MAX))
            .bind(i64::try_from(output_tokens).unwrap_or(i64::MAX))
            .bind(cost)
            .bind(cut)
            .bind(read_at)
            .execute(tx.conn())
            .await?;
            true
        }
        Outcome::Wait {
            until,
            status,
            reason,
            tried,
        } => {
            sqlx::query(
                "UPDATE telemetry.evaluations
                    SET status = $3, reason = $4, due_at = $5,
                        attempts = attempts + CASE WHEN $6 THEN 1 ELSE 0 END, lease_id = NULL
                  WHERE id = $1 AND lease_id = $2",
            )
            .bind(leased.id)
            .bind(leased.lease_id)
            .bind(status)
            .bind(reason)
            .bind(until)
            .bind(tried)
            .execute(tx.conn())
            .await?;
            false
        }
        Outcome::Failed { reason, read_at } => {
            sqlx::query(
                "UPDATE telemetry.evaluations
                    SET status = 'failed', reason = $3, read_at = COALESCE($4, read_at),
                        attempts = attempts + 1, settled_at = now(), lease_id = NULL
                  WHERE id = $1 AND lease_id = $2",
            )
            .bind(leased.id)
            .bind(leased.lease_id)
            .bind(reason)
            .bind(read_at)
            .execute(tx.conn())
            .await?;
            true
        }
    };
    tx.commit().await?;
    Ok(settled)
}

/// When a try that failed is tried again: a minute, doubling, up to an hour.
fn backoff(attempts: i16) -> DateTime<Utc> {
    let minutes = 1_i64 << attempts.clamp(0, 6);
    Utc::now() + chrono::Duration::minutes(minutes.min(60))
}

/// The next Monday, 00:00 UTC.
fn next_monday(now: DateTime<Utc>) -> DateTime<Utc> {
    let days = 7 - u64::from(now.weekday().num_days_from_monday());
    let day = now
        .date_naive()
        .checked_add_days(Days::new(days))
        .unwrap_or(now.date_naive());
    Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0).unwrap_or_default())
}

/// The last Monday, 00:00 UTC.
fn this_monday(now: DateTime<Utc>) -> DateTime<Utc> {
    next_monday(now) - chrono::Duration::days(7)
}

/// The evaluator version an evaluation names, and its connection's row.
#[derive(sqlx::FromRow)]
struct Judge {
    name: String,
    messages: Jsonb<Vec<Message>>,
    score: Jsonb<ScoreSpec>,
    reasoning: bool,
    connection_id: Option<Uuid>,
    model: Option<String>,
    temperature: Option<f64>,
    weekly_limit: Option<f64>,
    resolved: Option<Uuid>,
    provider: Option<ModelProvider>,
    models: Option<Vec<String>>,
    refused_at: Option<DateTime<Utc>>,
    pointers: Option<Jsonb<BTreeMap<String, String>>>,
}

/// One evaluation worked: what it judged read without a chain row, judged
/// by its version's model, its score written and its trace sent.
async fn judge(state: &Arc<AppState>, leased: &Leased) -> Result<Outcome, TelmoniError> {
    let project_id = &leased.project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    if db::content_mode(&mut tx, project_id).await? != ContentMode::On {
        tx.commit().await?;
        return Ok(paused("its project keeps no content"));
    }
    let judge: Option<Judge> = sqlx::query_as(
        "SELECT e.name, e.messages, e.score, e.reasoning, e.connection_id, e.model, e.temperature,
                e.weekly_limit, c.id AS resolved, c.provider, c.models, c.refused_at, r.pointers
           FROM telemetry.evaluators e
           LEFT JOIN telemetry.model_connections c
             ON c.project_id = e.project_id
            AND (c.id = e.connection_id OR (e.connection_id IS NULL AND c.is_default))
           LEFT JOIN telemetry.evaluation_rules r ON r.id = $4
          WHERE e.project_id = $1 AND e.id = $2 AND e.version = $3",
    )
    .bind(project_id)
    .bind(leased.evaluator_id)
    .bind(leased.evaluator_version)
    .bind(leased.rule_id)
    .fetch_optional(tx.conn())
    .await?;
    let Some(judge) = judge else {
        tx.commit().await?;
        return Ok(Outcome::Failed {
            reason: "its evaluator was deleted".into(),
            read_at: None,
        });
    };
    if let Some(limit) = judge.weekly_limit {
        let spent: Option<f64> = sqlx::query_scalar(
            "SELECT sum(cost) FROM telemetry.evaluations
              WHERE project_id = $1 AND evaluator_id = $2 AND settled_at >= $3",
        )
        .bind(project_id)
        .bind(leased.evaluator_id)
        .bind(this_monday(Utc::now()))
        .fetch_one(tx.conn())
        .await?;
        if spent.unwrap_or(0.0) >= limit {
            tx.commit().await?;
            return Ok(Outcome::Wait {
                until: next_monday(Utc::now()),
                status: "delayed",
                reason: "its evaluator's weekly spend limit is reached".into(),
                tried: false,
            });
        }
    }
    tx.commit().await?;
    let (Some(connection_id), Some(provider)) = (judge.resolved, judge.provider) else {
        return Ok(paused(if judge.connection_id.is_some() {
            "its model connection was deleted"
        } else {
            "the project has no default model connection"
        }));
    };
    if judge.refused_at.is_some() {
        return Ok(paused(
            "its model connection's key was refused; save the connection again",
        ));
    }
    let model = judge
        .model
        .clone()
        .or_else(|| judge.models.as_ref().and_then(|m| m.first().cloned()))
        .unwrap_or_default();
    let prompt = Prompt {
        messages: judge.messages.0.clone(),
        score: judge.score.0.clone(),
        reasoning: judge.reasoning,
    };

    let read_at = Utc::now();
    let Some((filled, cut, trace_id)) = filled(state, leased).await? else {
        return Ok(Outcome::Failed {
            reason: "what it judges is no longer kept".into(),
            read_at: Some(read_at),
        });
    };
    let pointers = judge.pointers.map(|p| p.0).unwrap_or_default();
    let filled = evaluators::narrowed(filled, &pointers);
    let connection =
        crate::handler::model_connections::open(state, project_id, connection_id).await?;
    let (system, messages) = prompt.rendered(&filled);
    let schema = prompt.schema();
    let started = Utc::now();
    let ask = Ask {
        model: &model,
        system: &system,
        messages: &messages,
        schema: &schema,
        temperature: judge.temperature,
        max_tokens: ANSWER_TOKENS_MAX,
    };
    let judged = match model::judge(&state.egress, &connection, &ask).await {
        Ok(judged) => judged,
        Err(refused) => return refusal(state, leased, connection_id, refused, read_at).await,
    };
    let ended = Utc::now();
    let cost = price(state, project_id, provider, &model, judged.usage).await;
    // Its reasoning is content: kept in its own trace's answer, never the score.
    let (judgement, _) = match prompt.judgement(&judged.answer) {
        Ok(read) => read,
        Err(why) => {
            return Ok(Outcome::Failed {
                reason: why,
                read_at: Some(read_at),
            });
        }
    };
    let target = match (
        leased.target,
        leased.run_id,
        &leased.span_id,
        &leased.session_id,
    ) {
        (EvaluationTarget::Run, Some(run), _, _) => TargetKey::Run(run),
        (EvaluationTarget::Span, Some(run), Some(span), _) => TargetKey::Span(run, span.clone()),
        (EvaluationTarget::Session, _, _, Some(session)) => TargetKey::Session(session.clone()),
        _ => {
            return Ok(Outcome::Failed {
                reason: "its target is unreadable".into(),
                read_at: Some(read_at),
            });
        }
    };
    let trace_run_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, leased.id.as_bytes());
    let written = own_trace(
        state,
        leased,
        &OwnTrace {
            run_id: trace_run_id,
            evaluator: &judge.name,
            provider,
            model: &model,
            system: &system,
            messages: &messages,
            answer: &judged.answer,
            usage: judged.usage,
            started,
            ended,
        },
    )
    .await;
    if let Err(error) = &written {
        tracing::warn!(evaluation_id = %leased.id, project_id = %project_id, error = %error,
            "telemetry: an evaluation's own trace was not written; its score stands");
    }
    let score_id = crate::scores::evaluator_score_id(project_id, &target, leased.evaluator_id);
    let (run_id, trace, span, session) = match &target {
        TargetKey::Run(run) => (*run, trace_id.clone(), String::new(), String::new()),
        TargetKey::Span(run, span) => (*run, trace_id.clone(), span.clone(), String::new()),
        TargetKey::Session(session) => (Uuid::nil(), String::new(), String::new(), session.clone()),
    };
    let row = ScoreRow {
        project_id: project_id.clone(),
        received_at: Utc::now().trunc_subsecs(6),
        score_id,
        target: target.kind(),
        run_id,
        trace_id: trace,
        span_id: span,
        session_id: session,
        name: judgement.name.clone(),
        data_type: judgement.data_type,
        value: judgement.value.unwrap_or(0.0),
        categories: judgement.categories.clone(),
        range_min: judgement.range_min,
        range_max: judgement.range_max,
        source: ScoreSource::Evaluator,
        scorer: leased.evaluator_id.to_string(),
        scorer_version: leased.evaluator_version.to_string(),
        is_deleted: 0,
    };
    state
        .store
        .insert_scores_once(
            std::slice::from_ref(&row),
            &format!("evaluation:{}", leased.id),
        )
        .await
        .map_err(|e| crate::store::failed(&e, "an evaluation's score", "tried again shortly"))?;
    tracing::info!(evaluation_id = %leased.id, project_id = %project_id,
        evaluator_id = %leased.evaluator_id, "telemetry: an evaluation done");
    Ok(Outcome::Done {
        score_id,
        trace_run_id: written.is_ok().then_some(trace_run_id),
        provider: provider.to_string(),
        model,
        input_tokens: judged.usage.input_tokens,
        output_tokens: judged.usage.output_tokens,
        cost,
        cut,
        read_at,
    })
}

/// Waiting for what pauses it: looked at again in a few minutes.
fn paused(why: &str) -> Outcome {
    Outcome::Wait {
        until: Utc::now() + chrono::Duration::seconds(PAUSED_SECS),
        status: "pending",
        reason: format!("paused: {why}"),
        tried: false,
    }
}

/// What the provider's refusal makes of an evaluation: a refused key pauses
/// every evaluator on the connection and tells the owners once; a rate limit
/// delays it by the provider's word, else by backoff; any other failure is
/// tried again until [`ATTEMPTS_MAX`], then failed with the provider's words.
async fn refusal(
    state: &AppState,
    leased: &Leased,
    connection_id: Uuid,
    refused: Refused,
    read_at: DateTime<Utc>,
) -> Result<Outcome, TelmoniError> {
    match refused {
        Refused::Unauthorized { .. } => {
            let mut tx = project_scope(&state.db, &leased.project_id).await?;
            let marked = sqlx::query(
                "UPDATE telemetry.model_connections SET refused_at = now(), refused_reason = $3
                  WHERE project_id = $1 AND id = $2 AND refused_at IS NULL",
            )
            .bind(&leased.project_id)
            .bind(connection_id)
            .bind(refused.sentence())
            .execute(tx.conn())
            .await?
            .rows_affected();
            tx.commit().await?;
            if marked > 0 {
                tell_refused(state, &leased.organization_id, connection_id).await;
            }
            Ok(paused(
                "its model connection's key was refused; save the connection again",
            ))
        }
        Refused::RateLimited { retry_after } => Ok(Outcome::Wait {
            until: retry_after.map_or_else(
                || backoff(leased.attempts),
                |wait| Utc::now() + chrono::Duration::from_std(wait).unwrap_or_default(),
            ),
            status: "delayed",
            reason: refused.sentence(),
            tried: true,
        }),
        other if leased.attempts + 1 >= ATTEMPTS_MAX => Ok(Outcome::Failed {
            reason: other.sentence(),
            read_at: Some(read_at),
        }),
        other => Ok(Outcome::Wait {
            until: backoff(leased.attempts),
            status: "pending",
            reason: other.sentence(),
            tried: true,
        }),
    }
}

/// The organization's owners and admins told, once a refusal, that a
/// connection's evaluators wait for its key.
async fn tell_refused(state: &AppState, organization_id: &OrganizationId, connection_id: Uuid) {
    let dedup = format!("evaluators:{connection_id}");
    let alert = OrganizationAlert {
        title: "An evaluator's model key was refused",
        body: "The provider refused a project's model connection, so the evaluators judging with it are paused. Save the connection again with a key the provider takes, on the project's Settings page.",
        metadata: json!({ "lane": "evaluators", "connection_id": connection_id }),
        dedup_key: Some(&dedup),
    };
    seam::tell_owners(
        state.notifications.as_ref(),
        state.auth.as_ref(),
        organization_id,
        alert,
        "/",
    )
    .await;
}

/// What a call cost at its model's price, where the table has one.
async fn price(
    state: &AppState,
    project_id: &ProjectId,
    provider: ModelProvider,
    model: &str,
    usage: model::Usage,
) -> Option<f64> {
    let provider = priced_as(provider)?;
    let book = PriceBook::read_for(
        &state.db,
        std::slice::from_ref(project_id),
        &[(provider.to_owned(), model.to_owned())],
    )
    .await
    .ok()?;
    book.price(provider, model, Utc::now()).map(|price| {
        price.cost(Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            ..Usage::default()
        })
    })
}

/// The variables an evaluation's target fills, whether a field it read was
/// cut to fit an export, and its trace's id; none where its spans are gone.
async fn filled(
    state: &AppState,
    leased: &Leased,
) -> Result<Option<(BTreeMap<&'static str, String>, bool, String)>, TelmoniError> {
    let projects = Projects::one(leased.project_id.clone());
    let failed = |e: clickhouse::error::Error| {
        crate::store::failed(&e, "an evaluation's target", "tried again shortly")
    };
    let cut_in = |spans: &[SpanRow]| {
        spans
            .iter()
            .any(|s| s.attributes.iter().any(|(k, _)| k == contract::CONTENT_CUT))
    };
    match (leased.target, leased.run_id, &leased.session_id) {
        (EvaluationTarget::Run | EvaluationTarget::Span, Some(run), _) => {
            let spans = traces::spans(&state.store, &projects, run, SPANS_MAX, u32::MAX)
                .await
                .map_err(failed)?;
            let Some(first) = spans.first() else {
                return Ok(None);
            };
            let trace_id = first.trace_id.clone();
            if leased.target == EvaluationTarget::Run {
                let content =
                    content::open_for_evaluation(state, &leased.project_id, &[run], None).await?;
                return Ok(Some((
                    evaluators::of_run(&spans, &content),
                    cut_in(&spans),
                    trace_id,
                )));
            }
            let Some(span) = spans
                .iter()
                .find(|s| Some(&s.span_id) == leased.span_id.as_ref())
            else {
                return Ok(None);
            };
            let content = content::open_for_evaluation(
                state,
                &leased.project_id,
                &[run],
                Some(std::slice::from_ref(&span.span_id)),
            )
            .await?;
            Ok(Some((
                evaluators::of_span(span, &content),
                cut_in(std::slice::from_ref(span)),
                trace_id,
            )))
        }
        (EvaluationTarget::Session, _, Some(session)) => {
            let filter = crate::traces::TraceFilter {
                session_ids: vec![session.clone()],
                days: traces::MAX_DAYS,
                ..crate::traces::TraceFilter::default()
            };
            let runs = traces::list::<Traces>(
                &state.store,
                &projects,
                &filter,
                (&ListSort::Column(TraceSort::Started), SortDirection::Asc),
                SESSION_RUNS_MAX,
                0,
            )
            .await
            .map_err(failed)?;
            if runs.is_empty() {
                return Ok(None);
            }
            let ids: Vec<Uuid> = runs.iter().map(|r| r.run_id).collect();
            let content =
                content::open_for_evaluation(state, &leased.project_id, &ids, None).await?;
            let mut turns = Vec::with_capacity(runs.len());
            let mut cut = false;
            for run in &ids {
                let spans = traces::spans(&state.store, &projects, *run, SPANS_MAX, u32::MAX)
                    .await
                    .map_err(failed)?;
                cut |= cut_in(&spans);
                let mine = content
                    .iter()
                    .filter(|c| c.run_id == *run)
                    .cloned()
                    .collect();
                turns.push((spans, mine));
            }
            Ok(Some((evaluators::of_session(&turns), cut, String::new())))
        }
        _ => Ok(None),
    }
}

/// What an evaluation's own trace holds.
struct OwnTrace<'a> {
    run_id: Uuid,
    evaluator: &'a str,
    provider: ModelProvider,
    model: &'a str,
    system: &'a str,
    messages: &'a [(model::Role, String)],
    answer: &'a Value,
    usage: model::Usage,
    started: DateTime<Utc>,
    ended: DateTime<Utc>,
}

/// The evaluation's own trace — a run and its one model call, the prompt
/// sent and the answer kept as the project's content — written through
/// ingest's own path, under [`contract::EVALUATOR_ENVIRONMENT`], once under
/// the evaluation's id.
async fn own_trace(
    state: &Arc<AppState>,
    leased: &Leased,
    trace: &OwnTrace<'_>,
) -> Result<(), TelmoniError> {
    let bytes = own_export(trace).to_string().into_bytes();
    let key = ActingKey {
        token_id: Uuid::nil(),
        organization_id: leased.organization_id.clone(),
        project_id: leased.project_id.clone(),
        scopes: Vec::new(),
    };
    crate::handler::ingest::write(
        state,
        &key,
        Encoding::Json,
        &bytes,
        None,
        &format!("{}:evaluation:{}", leased.project_id.as_str(), leased.id),
        Some(contract::EVALUATOR_ENVIRONMENT),
    )
    .await
    .map(drop)
}

/// The OTLP export of an evaluation's own trace, as an SDK would send it.
#[must_use]
fn own_export(trace: &OwnTrace<'_>) -> Value {
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let id = trace.run_id.as_bytes();
    let trace_id = hex(id);
    let run_span = hex(id.get(..8).unwrap_or(&[0; 8]));
    let call_span = hex(id.get(8..).unwrap_or(&[0; 8]));
    let nanos = |at: DateTime<Utc>| at.timestamp_nanos_opt().unwrap_or_default().to_string();
    let text = |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let int =
        |key: &str, value: u64| json!({ "key": key, "value": { "intValue": value.to_string() } });
    let mut sent =
        vec![json!({ "role": "system", "parts": [{ "type": "text", "content": trace.system }] })];
    sent.extend(trace.messages.iter().map(|(role, content)| {
        json!({
            "role": match role { model::Role::User => "user", model::Role::Assistant => "assistant" },
            "parts": [{ "type": "text", "content": content }],
        })
    }));
    let input = Value::Array(sent).to_string();
    let output = json!([{ "role": "assistant", "parts": [{ "type": "text", "content": trace.answer.to_string() }] }]).to_string();
    let agent = format!("evaluator: {}", trace.evaluator);
    let common = |operation: &str| {
        vec![
            text(contract::RUN_ID, &trace.run_id.to_string()),
            text(contract::AGENT, &agent),
            text(contract::OPERATION, operation),
        ]
    };
    let mut run = common("invoke_agent");
    run.push(text("gen_ai.input.messages", &input));
    run.push(text("gen_ai.output.messages", &output));
    let mut call = common("chat");
    call.extend([
        text(
            contract::PROVIDER,
            priced_as(trace.provider).unwrap_or("openai"),
        ),
        text(contract::REQUEST_MODEL, trace.model),
        int(contract::INPUT_TOKENS, trace.usage.input_tokens),
        int(contract::OUTPUT_TOKENS, trace.usage.output_tokens),
        text("gen_ai.input.messages", &input),
        text("gen_ai.output.messages", &output),
    ]);
    json!({
        "resourceSpans": [{
            "resource": { "attributes": [text("service.name", contract::EVALUATOR_ENVIRONMENT)] },
            "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": [
                {
                    "traceId": trace_id, "spanId": run_span, "name": format!("invoke_agent {agent}"),
                    "startTimeUnixNano": nanos(trace.started), "endTimeUnixNano": nanos(trace.ended),
                    "attributes": run,
                },
                {
                    "traceId": trace_id, "spanId": call_span, "parentSpanId": run_span,
                    "name": format!("chat {}", trace.model),
                    "startTimeUnixNano": nanos(trace.started), "endTimeUnixNano": nanos(trace.ended),
                    "attributes": call,
                },
            ]}],
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A target is sampled alike every time, and a rate judges about its share.
    #[test]
    fn a_target_is_sampled_alike_every_time() {
        assert!(sampled("run/a", 1.0));
        assert_eq!(sampled("run/a", 0.5), sampled("run/a", 0.5));
        let judged = (0..2000)
            .filter(|n| sampled(&format!("run/{n}"), 0.25))
            .count();
        assert!((400..600).contains(&judged), "{judged}");
    }

    /// A weekly limit lifts on Monday, 00:00 UTC.
    #[test]
    fn a_week_runs_from_monday() {
        let wednesday = Utc
            .with_ymd_and_hms(2026, 11, 4, 15, 30, 0)
            .single()
            .expect("a time");
        assert_eq!(
            next_monday(wednesday),
            Utc.with_ymd_and_hms(2026, 11, 9, 0, 0, 0)
                .single()
                .expect("a time")
        );
        assert_eq!(
            this_monday(wednesday),
            Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0)
                .single()
                .expect("a time")
        );
        let monday = Utc
            .with_ymd_and_hms(2026, 11, 2, 0, 0, 0)
            .single()
            .expect("a time");
        assert_eq!(this_monday(monday), monday);
    }

    /// An evaluation's own trace is a run and its one model call, in one trace.
    #[test]
    fn its_own_trace_is_a_run_and_a_model_call() {
        let at = Utc
            .with_ymd_and_hms(2026, 11, 2, 12, 0, 0)
            .single()
            .expect("a time");
        let export = own_export(&OwnTrace {
            run_id: Uuid::nil(),
            evaluator: "Relevant",
            provider: ModelProvider::Openai,
            model: "gpt-x",
            system: "judge",
            messages: &[(model::Role::User, "q".into())],
            answer: &json!({ "score": true }),
            usage: model::Usage {
                input_tokens: 9,
                output_tokens: 3,
            },
            started: at,
            ended: at,
        });
        let spans = &export["resourceSpans"][0]["scopeSpans"][0]["spans"];
        assert_eq!(spans.as_array().map(Vec::len), Some(2));
        assert_eq!(spans[1]["parentSpanId"], spans[0]["spanId"]);
        assert_eq!(spans[0]["traceId"], spans[1]["traceId"]);
    }
}
