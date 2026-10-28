//! `telemetry.rules`: a project's alerts, read and written in its own scope,
//! counted across its organization's projects, and leased, settled and held
//! to the plan's number in the module's lane, which spans tenants.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{HasProject, Maintenance, ProjectAndOrganization, Scoped};
use telmoni_shared::{OrganizationId, ProjectId, Severity};

use crate::db::TelemetryLane;
use crate::rules::{Definition, Rule};

/// An alert as its row keeps it: its definition, whether and why it is
/// paused, and its last evaluation.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RuleRow {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub organization_id: OrganizationId,
    #[sqlx(flatten)]
    pub definition: Definition,
    /// Who paused it, and when; none while it runs.
    pub paused: Option<PausedBy>,
    pub paused_at: Option<DateTime<Utc>>,
    /// Its value at its last evaluation, none where the window held no span.
    pub value: Option<f64>,
    pub evaluated_at: Option<DateTime<Utc>>,
    /// Where it stands, since when, and when that was last told.
    pub severity: Severity,
    pub severity_since: DateTime<Utc>,
    pub told_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The definition's eleven columns bound, in the order the insert and the
/// update name them after their own.
trait BindDefinition<'q> {
    fn bind_definition(self, definition: &'q Definition) -> Self;
}

impl<'q> BindDefinition<'q>
    for sqlx::query::QueryAs<'q, sqlx::Postgres, RuleRow, sqlx::postgres::PgArguments>
{
    fn bind_definition(self, definition: &'q Definition) -> Self {
        self.bind(&definition.name)
            .bind(definition.kind)
            .bind(definition.measure)
            .bind(definition.aggregation)
            .bind(sqlx::types::Json(&definition.filters))
            .bind(definition.window)
            .bind(definition.operator)
            .bind(definition.alert_threshold)
            .bind(definition.warning_threshold)
            .bind(definition.missing_data)
            .bind(definition.renotify_minutes)
    }
}

/// The columns a [`RuleRow`] is read from, in its order.
const RULE_COLUMNS: &str = "id, project_id, organization_id, name, kind, measure, aggregation, \
    filters, lookback, operator, alert_threshold, warning_threshold, missing_data, \
    renotify_minutes, paused, paused_at, value, evaluated_at, severity, severity_since, told_at, \
    created_at, updated_at";

/// Who paused an alert: a member, by hand, or its plan, on a fall past its
/// number, which lifts its own once there is room again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum PausedBy {
    Person,
    Plan,
}

telmoni_shared::wire_enum!(PausedBy {
    Person => "person",
    Plan => "plan",
});

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

/// How many of the organization's alerts were made before `id`, the oldest
/// first as [`hold_to`] ranks them: what its plan holds ahead of it.
pub async fn organization_ahead(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    organization_id: &OrganizationId,
    id: Uuid,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM telemetry.rules AS other, telemetry.rules AS this
          WHERE this.id = $2 AND other.organization_id = $1
            AND (other.created_at, other.id) < (this.created_at, this.id)",
    )
    .bind(organization_id)
    .bind(id)
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
    .bind_definition(&rule.definition)
    .fetch_one(tx.conn())
    .await
}

/// One of the project's alerts, saved over: its definition anew, its last
/// evaluation forgotten, since it answered another question, and its lease
/// let go, so a replica holding it in a batch leaves it to the next pass.
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
                lease_id = NULL, leased_until = NULL, updated_at = now()
          WHERE project_id = $1 AND id = $2
         RETURNING {RULE_COLUMNS}"
    )))
    .bind(project_id)
    .bind(id)
    .bind_definition(&rule.definition)
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

/// A lease held on, unsettled, until `secs` from now: the alert waits for
/// the next pass, whichever replica runs it.
pub async fn defer(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    id: Uuid,
    lease_id: Uuid,
    secs: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE telemetry.rules SET leased_until = now() + make_interval(secs => $3)
          WHERE id = $1 AND lease_id = $2",
    )
    .bind(id)
    .bind(lease_id)
    .bind(secs)
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
        "SELECT DISTINCT organization_id FROM telemetry.rules WHERE paused = $1
          ORDER BY organization_id",
    )
    .bind(PausedBy::Plan)
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
            SET paused = CASE WHEN $2::bigint IS NOT NULL AND ranked.place > $2 THEN $3::text END,
                paused_at = CASE WHEN $2::bigint IS NOT NULL AND ranked.place > $2 THEN now() END,
                severity = CASE WHEN $2::bigint IS NOT NULL AND ranked.place > $2
                                THEN 'paused' ELSE 'unknown' END,
                severity_since = now(),
                -- Lifted, it reads as never evaluated, as a resume leaves it.
                value = CASE WHEN $2::bigint IS NOT NULL AND ranked.place > $2
                             THEN rule.value END,
                evaluated_at = CASE WHEN $2::bigint IS NOT NULL AND ranked.place > $2
                                    THEN rule.evaluated_at END,
                -- An evaluation in flight on another replica settles nothing.
                lease_id = NULL, leased_until = NULL
           FROM ranked
          WHERE rule.id = ranked.id
            AND ((rule.paused IS NULL AND $2::bigint IS NOT NULL AND ranked.place > $2)
              OR (rule.paused = $3 AND ($2::bigint IS NULL OR ranked.place <= $2)))",
    )
    .bind(organization_id)
    .bind(limit)
    .bind(PausedBy::Plan)
    .execute(tx.conn())
    .await?
    .rows_affected())
}

/// A connector an alert links, as its row keeps it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkRow {
    pub rule_id: Uuid,
    pub connection_id: Uuid,
    pub level: telmoni_shared::RuleLevel,
    pub hours: Option<sqlx::types::Json<crate::rules::Hours>>,
}

impl LinkRow {
    /// The link as the lanes and the loop read it.
    #[must_use]
    pub fn link(self) -> crate::rules::Link {
        crate::rules::Link {
            connection_id: self.connection_id,
            level: self.level,
            hours: self.hours.map(|hours| hours.0),
        }
    }
}

/// The connectors the project's alerts link, those of `rule_ids` alone.
pub async fn links<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    rule_ids: &[Uuid],
) -> sqlx::Result<Vec<LinkRow>> {
    sqlx::query_as(
        "SELECT rule_id, connection_id, level, hours FROM telemetry.rule_connectors
          WHERE project_id = $1 AND rule_id = ANY($2)
          ORDER BY rule_id, connection_id",
    )
    .bind(project_id)
    .bind(rule_ids)
    .fetch_all(tx.conn())
    .await
}

/// The connectors one alert links, read in the module's lane by the loop.
pub async fn links_of(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    rule_id: Uuid,
) -> sqlx::Result<Vec<LinkRow>> {
    sqlx::query_as(
        "SELECT rule_id, connection_id, level, hours FROM telemetry.rule_connectors
          WHERE rule_id = $1 ORDER BY connection_id",
    )
    .bind(rule_id)
    .fetch_all(tx.conn())
    .await
}

/// The alert's connectors, written whole in place of those it linked.
pub async fn write_links<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    rule_id: Uuid,
    links: &[crate::rules::Link],
) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM telemetry.rule_connectors WHERE project_id = $1 AND rule_id = $2")
        .bind(project_id)
        .bind(rule_id)
        .execute(tx.conn())
        .await?;
    for link in links {
        sqlx::query(
            "INSERT INTO telemetry.rule_connectors (rule_id, connection_id, project_id, level, hours)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(rule_id)
        .bind(link.connection_id)
        .bind(project_id)
        .bind(link.level)
        .bind(link.hours.as_ref().map(sqlx::types::Json))
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// The key an alert's notices are routed under, which counts each linked
/// connector's failed deliveries for it alone.
#[must_use]
pub fn route_key(rule_id: Uuid) -> String {
    format!("telemetry:rule:{rule_id}")
}

/// One of the project's alerts paused by a person, where it ran or its plan
/// paused it: no longer evaluated, its severity `paused`. Answers it as it
/// stands, none where the project has no such alert.
pub async fn pause<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<Option<RuleRow>> {
    sqlx::query(
        "UPDATE telemetry.rules
            SET paused = $3, paused_at = now(), severity = 'paused', severity_since = now(),
                lease_id = NULL, leased_until = NULL
          WHERE project_id = $1 AND id = $2 AND paused IS DISTINCT FROM $3",
    )
    .bind(project_id)
    .bind(id)
    .bind(PausedBy::Person)
    .execute(tx.conn())
    .await?;
    rule(tx, project_id, id).await
}

/// One of the project's paused alerts running again: due at once, its
/// severity `unknown` until its first evaluation.
pub async fn resume<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<Option<RuleRow>> {
    sqlx::query(
        "UPDATE telemetry.rules
            SET paused = NULL, paused_at = NULL, severity = 'unknown', severity_since = now(),
                value = NULL, evaluated_at = NULL
          WHERE project_id = $1 AND id = $2 AND paused IS NOT NULL",
    )
    .bind(project_id)
    .bind(id)
    .execute(tx.conn())
    .await?;
    rule(tx, project_id, id).await
}
