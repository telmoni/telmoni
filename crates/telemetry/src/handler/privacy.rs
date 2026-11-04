//! A project's privacy posture, `GET /internal/v1/telemetry/privacy`, whose
//! `retention` the project's Settings reads: what its spans arrived with over
//! the last [`SENT_DAYS`] by the field inventory's rows, dropped content among
//! them, what its evaluators read of its content over them, by day, and how
//! long its spans and the hourly totals that outlive them are kept.
//! Every role on the project `x-project-id` names reads it; its owners and
//! admins choose its retention, `PUT /internal/v1/telemetry/retention`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use chrono::{DateTime, Duration, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use serde_json::json;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{AuditAction, TelmoniError, TelmoniResourceKind};

use crate::handler::chain;
use crate::retention::{ROLLUP_RETENTION_MONTHS, SPAN_RETENTION_DAYS};
use crate::store::{self, Projects, micros};
use crate::{AppState, db};

/// The days the page counts back over.
pub const SENT_DAYS: u32 = 30;

/// The spans received in the window, and how many carried each row: by the
/// partition's day and `received_at_idx`.
const SENT_SQL: &str = "SELECT count() AS spans,
            sumMap(sent, arrayWithConstant(length(sent), toUInt64(1))) AS sent
       FROM telemetry.spans
      WHERE project_id IN {projects:Array(String)}
        AND received_at >= {since:DateTime64(6, 'UTC')}";

#[derive(Row, Deserialize)]
struct Counted {
    spans: u64,
    sent: (Vec<String>, Vec<u64>),
}

/// What the lane answers.
#[derive(Debug, Serialize)]
pub struct Posture {
    /// [`SENT_DAYS`].
    pub days: u32,
    /// Where they begin, which the page's list of reads starts from too.
    pub since: DateTime<Utc>,
    /// The spans received in them.
    pub spans: u64,
    /// Each inventory row a span arrived with, most spans first.
    pub sent: Vec<Sent>,
    /// What each evaluator read of the project's content in them, a row a
    /// day an evaluator a provider, the newest day first: each read the
    /// evaluation's own row records, never a chain row each.
    pub evaluator_reads: Vec<EvaluatorReads>,
    /// How long what the spans leave is kept.
    pub retention: Retention,
}

/// One inventory row, and the spans that arrived with it.
#[derive(Debug, Serialize)]
pub struct Sent {
    /// The row's `attribute`, as the inventory spells it.
    pub attribute: String,
    pub spans: u64,
}

/// One evaluator's reads of content on one day, sent to one provider.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct EvaluatorReads {
    /// The day, in UTC.
    pub day: chrono::NaiveDate,
    pub evaluator_id: uuid::Uuid,
    /// Its newest version's name; none once it is deleted.
    pub evaluator: Option<String>,
    /// The provider its model's connection sent the content to.
    pub provider: Option<String>,
    pub reads: i64,
}

/// The most rows of reads the page lists.
pub const EVALUATOR_READS_MAX: i64 = 500;

/// The windows `telmoni-server rotate` holds for the project.
#[derive(Debug, Serialize)]
pub struct Retention {
    /// Spans, and the content an `on` project keeps with them: the project's
    /// own choice, else [`SPAN_RETENTION_DAYS`].
    pub span_days: i64,
    /// The most it may choose: the longest sold.
    pub max_span_days: i64,
    /// The hourly totals, which name no user and no tag, the same for every
    /// project.
    pub rollup_months: u32,
}

impl Retention {
    fn of(chosen: Option<i16>) -> Self {
        Self {
            span_days: chosen.map_or(SPAN_RETENTION_DAYS, i64::from),
            max_span_days: SPAN_RETENTION_DAYS,
            rollup_months: ROLLUP_RETENTION_MONTHS,
        }
    }
}

/// The body of a change: the days to keep, or none for the longest sold.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionChange {
    pub retention_days: Option<i64>,
}

/// `GET /internal/v1/telemetry/privacy` — the project's posture.
pub async fn get_privacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Posture>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Project)?
        .project_id;
    let since = Utc::now() - Duration::days(i64::from(SENT_DAYS));
    let mut tx = project_scope(&state.db, project_id).await?;
    let chosen = db::retention_days(&mut tx, project_id).await?;
    let evaluator_reads: Vec<EvaluatorReads> = sqlx::query_as(
        "SELECT (e.read_at AT TIME ZONE 'UTC')::date AS day, e.evaluator_id,
                (SELECT v.name FROM telemetry.evaluators v
                  WHERE v.project_id = e.project_id AND v.id = e.evaluator_id
                  ORDER BY v.version DESC LIMIT 1) AS evaluator,
                e.provider, count(*) AS reads
           FROM telemetry.evaluations e
          WHERE e.project_id = $1 AND e.read_at >= $2
          GROUP BY day, e.evaluator_id, e.project_id, e.provider
          ORDER BY day DESC, reads DESC
          LIMIT $3",
    )
    .bind(project_id)
    .bind(since)
    .bind(EVALUATOR_READS_MAX)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    let counted: Counted = state
        .store
        .read(&Projects::one(project_id.clone()), SENT_SQL)
        .param("since", micros(since))
        .fetch_one()
        .await
        .map_err(|e| {
            store::failed(
                &e,
                "a project's privacy posture",
                "the span store did not answer; try again",
            )
        })?;
    let (attributes, spans) = counted.sent;
    let mut sent: Vec<Sent> = attributes
        .into_iter()
        .zip(spans)
        .map(|(attribute, spans)| Sent { attribute, spans })
        .collect();
    sent.sort_by(|a, b| {
        b.spans
            .cmp(&a.spans)
            .then_with(|| a.attribute.cmp(&b.attribute))
    });
    Ok(Json(Posture {
        days: SENT_DAYS,
        since,
        spans: counted.spans,
        sent,
        evaluator_reads,
        retention: Retention::of(chosen),
    }))
}

/// `PUT /internal/v1/telemetry/retention` — how many days the project keeps
/// its spans and content, one up to the longest sold, or none for that;
/// decided under the organization's chain lock, the role checked again
/// there, and a change recorded, from and to, in the same transaction. The
/// nightly purge holds it; the hourly totals keep their months.
pub async fn put_retention(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(change): Json<RetentionChange>,
) -> Result<Json<Retention>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Project)?
        .project_id
        .clone();
    // The longest, named, is kept as none: the same days, and it follows
    // what is sold.
    let days = match change.retention_days {
        None | Some(SPAN_RETENTION_DAYS) => None,
        Some(days) if (1..SPAN_RETENTION_DAYS).contains(&days) => i16::try_from(days).ok(),
        Some(_) => {
            return Err(TelmoniError::BadRequest(format!(
                "a project keeps its spans 1 to {SPAN_RETENTION_DAYS} days"
            )));
        }
    };
    let auth = &*state.auth;
    chain::write(&state.db, &acting, &project_id, "retention", async |tx| {
        chain::held(auth, &acting, tx, Verb::Update, Resource::Project).await?;
        let before = db::set_retention_days(tx, &project_id, &acting.organization_id, days).await?;
        if before != days {
            emit_audit(
                tx,
                AuditEvent::new(
                    &acting.organization_id,
                    Actor::User(acting.user_id.as_str()),
                    AuditAction::Updated,
                    TelmoniResourceKind::Project,
                )
                .in_project(&project_id)
                .on(project_id.as_str())
                .details(
                    json!({ "retention_days": { "from": days_of(before), "to": days_of(days) } }),
                ),
            )
            .await?;
        }
        Ok(())
    })
    .await?;
    tracing::info!(project_id = %project_id, retention_days = days_of(days), "telemetry: a project's retention set");
    Ok(Json(Retention::of(days)))
}

/// The days a choice keeps, the longest sold for none.
fn days_of(chosen: Option<i16>) -> i64 {
    chosen.map_or(SPAN_RETENTION_DAYS, i64::from)
}
