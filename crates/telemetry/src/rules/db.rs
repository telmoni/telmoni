//! `telemetry.rules`: a project's alerts, read and written in its own scope,
//! counted across its organization's projects, and leased, settled and held
//! to the plan's number in the module's lane, which spans tenants.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{HasProject, Maintenance, ProjectAndOrganization, Scoped};
use telmoni_shared::{
    MissingData, OrganizationId, ProjectId, RuleAggregation, RuleMeasure, RuleOperator, RuleWindow,
    SpanKind,
};

use crate::db::TelemetryLane;
use crate::rules::{Rule, RuleFilters};

/// An alert as its row keeps it: its definition, whether and why it is
/// paused, and its last evaluation.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RuleRow {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub organization_id: OrganizationId,
    pub name: String,
    pub kind: SpanKind,
    pub measure: RuleMeasure,
    pub aggregation: RuleAggregation,
    pub filters: sqlx::types::Json<RuleFilters>,
    pub lookback: RuleWindow,
    pub operator: RuleOperator,
    pub alert_threshold: f64,
    pub warning_threshold: Option<f64>,
    pub missing_data: MissingData,
    pub renotify_minutes: Option<i32>,
    /// Who paused it — `person` or `plan` — and when; none while it runs.
    pub paused: Option<String>,
    pub paused_at: Option<DateTime<Utc>>,
    /// Its value at its last evaluation, none where the window held no span.
    pub value: Option<f64>,
    pub evaluated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The columns a [`RuleRow`] is read from, in its order.
const RULE_COLUMNS: &str = "id, project_id, organization_id, name, kind, measure, aggregation, \
    filters, lookback, operator, alert_threshold, warning_threshold, missing_data, \
    renotify_minutes, paused, paused_at, value, evaluated_at, created_at, updated_at";

/// Who paused an alert: a member, by hand, or its plan, on a fall past its
/// number, which lifts its own once there is room again.
pub const PAUSED_BY_PERSON: &str = "person";
pub const PAUSED_BY_PLAN: &str = "plan";

/// The project's alerts, by name.
pub async fn rules<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Vec<RuleRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {RULE_COLUMNS} FROM telemetry.rules
          WHERE project_id = $1
          ORDER BY lower(name), id"
    )))
    .bind(project_id)
    .fetch_all(tx.conn())
    .await
}

/// One of the project's alerts.
pub async fn rule<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<Option<RuleRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {RULE_COLUMNS} FROM telemetry.rules WHERE project_id = $1 AND id = $2"
    )))
    .bind(project_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await
}

/// How many alerts the organization holds across its projects, paused ones
/// among them: what its plan's number is held to.
pub async fn organization_count(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    organization_id: &OrganizationId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM telemetry.rules WHERE organization_id = $1")
        .bind(organization_id)
        .fetch_one(tx.conn())
        .await
}

/// A new alert of the project, filed under its organization.
pub async fn insert(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    id: Uuid,
    rule: &Rule,
) -> sqlx::Result<RuleRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO telemetry.rules (id, project_id, organization_id, name, kind, measure,
             aggregation, filters, lookback, operator, alert_threshold, warning_threshold,
             missing_data, renotify_minutes)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
         RETURNING {RULE_COLUMNS}"
    )))
    .bind(id)
    .bind(project_id)
    .bind(organization_id)
    .bind(&rule.name)
    .bind(rule.kind)
    .bind(rule.measure)
    .bind(rule.aggregation)
    .bind(sqlx::types::Json(&rule.filters))
    .bind(rule.window)
    .bind(rule.operator)
    .bind(rule.alert_threshold)
    .bind(rule.warning_threshold)
    .bind(rule.missing_data)
    .bind(rule.renotify_minutes)
    .fetch_one(tx.conn())
    .await
}

/// One of the project's alerts, saved over: its definition anew, and its
/// last evaluation forgotten, since it answered another question.
pub async fn update<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
    rule: &Rule,
) -> sqlx::Result<Option<RuleRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE telemetry.rules
            SET name = $3, kind = $4, measure = $5, aggregation = $6, filters = $7,
                lookback = $8, operator = $9, alert_threshold = $10, warning_threshold = $11,
                missing_data = $12, renotify_minutes = $13, value = NULL, evaluated_at = NULL,
                updated_at = now()
          WHERE project_id = $1 AND id = $2
         RETURNING {RULE_COLUMNS}"
    )))
    .bind(project_id)
    .bind(id)
    .bind(&rule.name)
    .bind(rule.kind)
    .bind(rule.measure)
    .bind(rule.aggregation)
    .bind(sqlx::types::Json(&rule.filters))
    .bind(rule.window)
    .bind(rule.operator)
    .bind(rule.alert_threshold)
    .bind(rule.warning_threshold)
    .bind(rule.missing_data)
    .bind(rule.renotify_minutes)
    .fetch_optional(tx.conn())
    .await
}

/// One of the project's alerts, gone. Answers whether there was one.
pub async fn delete<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<bool> {
    Ok(
        sqlx::query("DELETE FROM telemetry.rules WHERE project_id = $1 AND id = $2")
            .bind(project_id)
            .bind(id)
            .execute(tx.conn())
            .await?
            .rows_affected()
            > 0,
    )
}

/// Up to `limit` running alerts whose last evaluation is a minute old or
/// more, and that no lease holds, leased under `lease_id` for `lease_secs`:
/// locked as they are read (`FOR UPDATE SKIP LOCKED`), so replicas share the
/// loop without a leader and never take one alert twice.
pub async fn lease_due(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    every_secs: i64,
    lease_id: Uuid,
    lease_secs: i64,
    limit: i64,
) -> sqlx::Result<Vec<RuleRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE telemetry.rules SET lease_id = $2, leased_until = now() + make_interval(secs => $3)
          WHERE id IN (
                SELECT id FROM telemetry.rules
                 WHERE paused IS NULL
                   AND (evaluated_at IS NULL
                        OR evaluated_at <= now() - make_interval(secs => $1))
                   AND (leased_until IS NULL OR leased_until < now())
                 ORDER BY evaluated_at NULLS FIRST, id
                 LIMIT $4
                 FOR UPDATE SKIP LOCKED)
         RETURNING {RULE_COLUMNS}"
    )))
    .bind(every_secs)
    .bind(lease_id)
    .bind(lease_secs)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// An alert's evaluation, settled where the lease is still its own and the
/// alert reads as it did when it was leased: one saved over meanwhile is
/// evaluated again rather than given the old question's answer.
pub async fn settle(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    id: Uuid,
    lease_id: Uuid,
    updated_at: DateTime<Utc>,
    value: Option<f64>,
    evaluated_at: DateTime<Utc>,
) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "UPDATE telemetry.rules
            SET value = $4, evaluated_at = $5, lease_id = NULL, leased_until = NULL
          WHERE id = $1 AND lease_id = $2 AND updated_at = $3",
    )
    .bind(id)
    .bind(lease_id)
    .bind(updated_at)
    .bind(value)
    .bind(evaluated_at)
    .execute(tx.conn())
    .await?
    .rows_affected()
        > 0)
}

/// A lease let go unsettled: the alert is due again at the next pass.
pub async fn release(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    id: Uuid,
    lease_id: Uuid,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE telemetry.rules SET lease_id = NULL, leased_until = NULL
          WHERE id = $1 AND lease_id = $2",
    )
    .bind(id)
    .bind(lease_id)
    .execute(tx.conn())
    .await
    .map(|_| ())
}

/// The organizations whose plan paused any of their alerts: the ones a
/// pass holds to their number again whether or not one is due, so a plan
/// bought lifts its pauses.
pub async fn held_by_plan(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
) -> sqlx::Result<Vec<OrganizationId>> {
    sqlx::query_scalar(
        "SELECT DISTINCT organization_id FROM telemetry.rules WHERE paused = 'plan'
          ORDER BY organization_id",
    )
    .fetch_all(tx.conn())
    .await
}

/// The organization's alerts held to `limit`, the oldest first: those past
/// it paused by the plan, unless a person paused them already, and those
/// within it that the plan paused running again; with no limit, every one
/// the plan paused runs again. Answers how many changed.
pub async fn hold_to(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    organization_id: &OrganizationId,
    limit: Option<i64>,
) -> sqlx::Result<u64> {
    Ok(sqlx::query(
        "WITH ranked AS (
              SELECT id, row_number() OVER (ORDER BY created_at, id) AS place
                FROM telemetry.rules WHERE organization_id = $1)
         UPDATE telemetry.rules AS rule
            SET paused = CASE WHEN $2::bigint IS NOT NULL AND ranked.place > $2 THEN 'plan' END,
                paused_at = CASE WHEN $2::bigint IS NOT NULL AND ranked.place > $2 THEN now() END
           FROM ranked
          WHERE rule.id = ranked.id
            AND ((rule.paused IS NULL AND $2::bigint IS NOT NULL AND ranked.place > $2)
              OR (rule.paused = 'plan' AND ($2::bigint IS NULL OR ranked.place <= $2)))",
    )
    .bind(organization_id)
    .bind(limit)
    .execute(tx.conn())
    .await?
    .rows_affected())
}
