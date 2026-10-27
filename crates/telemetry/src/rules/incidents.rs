//! An alert's severity and its incidents. Each evaluation sets the severity
//! its value crosses to — `alert`, `warning` or `ok`; for a window that held
//! no span what the alert asks, read as 0, kept, or `no_data` — and moves the
//! alert's incident with it, in the transaction that settles the evaluation:
//! a breach opens one, or reopens the alert's last within an hour of its
//! recovery; an escalation follows it; a recovery resolves it; and, where
//! the alert asks, a window empty for as long as the window opens one of its
//! own. Each is told after the commit: an opening and a resolution in the
//! project's feed and to its connectors, keyed by the incident and the kind,
//! so an incident has one feed row of each; what it says again — a reopening,
//! an escalation, a second recovery, a reminder of a severity held as often
//! as the alert asks — to the connectors alone.

use chrono::{DateTime, TimeDelta, Utc};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{
    Binding, HasOrganization, HasProject, Maintenance, Scoped, maintenance_scope,
};
use telmoni_shared::{
    IncidentStatus, MissingData, NotificationKind, OrganizationId, ProjectId, RuleOperator,
    Severity, TelmoniError, UserId,
};

use crate::AppState;
use crate::db::TelemetryLane;
use crate::rules::db::RuleRow;

/// How long after its recovery an alert's breach reopens its incident
/// rather than opening another.
pub const REOPEN_WITHIN: TimeDelta = TimeDelta::hours(1);

/// The most runs an incident keeps of those its notices showed, the latest,
/// as the table's CHECK has it.
pub const RUNS_KEPT: i32 = 20;

/// An incident as its row keeps it, with its alert's name.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct IncidentRow {
    pub id: Uuid,
    pub rule_id: Uuid,
    pub rule_name: String,
    pub project_id: ProjectId,
    pub organization_id: OrganizationId,
    pub status: IncidentStatus,
    pub severity: Severity,
    pub value: Option<f64>,
    pub opened_at: DateTime<Utc>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub acknowledged_by: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by: Option<String>,
    pub reopened: i32,
    /// The connections its notices were queued to; the feed is not one.
    pub told_to: Vec<Uuid>,
    /// The runs its notices showed, oldest first.
    pub runs: Vec<Uuid>,
}

/// The columns an [`IncidentRow`] is read from, its alert joined as `rule`.
const INCIDENT_COLUMNS: &str = "incident.id, incident.rule_id, rule.name AS rule_name, \
    incident.project_id, incident.organization_id, incident.status, incident.severity, \
    incident.value, incident.opened_at, incident.acknowledged_at, incident.acknowledged_by, \
    incident.resolved_at, incident.resolved_by, incident.reopened, incident.told_to, \
    incident.runs";

/// The severity `value` crosses to under the alert's thresholds.
#[must_use]
pub fn crossed(operator: RuleOperator, alert: f64, warning: Option<f64>, value: f64) -> Severity {
    if operator.crosses(value, alert) {
        Severity::Alert
    } else if warning.is_some_and(|warning| operator.crosses(value, warning)) {
        Severity::Warning
    } else {
        Severity::Ok
    }
}

/// The severity an evaluation sets, from the one the alert stood at:
/// `value` crossed against its thresholds, or, for a window that held no
/// span, what its missing data asks.
#[must_use]
pub fn next_severity(rule: &RuleRow, value: Option<f64>) -> Severity {
    let cross = |value| {
        crossed(
            rule.operator,
            rule.alert_threshold,
            rule.warning_threshold,
            value,
        )
    };
    match (value, rule.missing_data) {
        (Some(value), _) => cross(value),
        (None, MissingData::Zero) => cross(0.0),
        (None, MissingData::Keep) => rule.severity,
        (None, MissingData::NoData | MissingData::NotifyNoData) => Severity::NoData,
    }
}

/// What one evaluation moves, and tells after its commit.
#[derive(Debug, Clone, PartialEq)]
pub struct Told {
    pub incident_id: Uuid,
    pub kind: NotificationKind,
    pub severity: Severity,
    /// The severity whose connectors hear it: its own, or for a resolution
    /// the incident's last.
    pub heard_at: Severity,
    /// Said again, to the connectors alone, whatever the feed holds.
    pub again: bool,
    pub reminder: bool,
    /// Written, and told to nobody: an alert falling from `alert` to
    /// `warning` follows its incident down in silence.
    pub silent: bool,
}

/// An evaluation settled with what its severity moves: written where the
/// lease and the definition it read still stand, with the alert's incident
/// opened, followed or resolved in the same transaction. Answers whether it
/// was settled, and what is to be told.
pub async fn settle(
    state: &AppState,
    rule: &RuleRow,
    lease_id: Uuid,
    value: Option<f64>,
    now: DateTime<Utc>,
) -> Result<(bool, Vec<Told>), TelmoniError> {
    let next = next_severity(rule, value);
    let since = if next == rule.severity {
        rule.severity_since
    } else {
        now
    };
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let open = open_of(&mut tx, rule.id).await?;
    let moved = moves(rule, next, since, open.as_ref(), now);
    let told_at = if moved.iter().any(|m| !m.silent) {
        Some(now)
    } else {
        rule.told_at
    };
    let settled = sqlx::query(
        "UPDATE telemetry.rules
            SET value = $4, evaluated_at = $5, severity = $6, severity_since = $7, told_at = $8,
                lease_id = NULL, leased_until = NULL
          WHERE id = $1 AND lease_id = $2 AND updated_at = $3",
    )
    .bind(rule.id)
    .bind(lease_id)
    .bind(rule.updated_at)
    .bind(value)
    .bind(now)
    .bind(next)
    .bind(since)
    .bind(told_at)
    .execute(tx.conn())
    .await?
    .rows_affected()
        > 0;
    if !settled {
        tx.rollback().await?;
        return Ok((false, Vec::new()));
    }
    let mut written = Vec::with_capacity(moved.len());
    for one in moved {
        written.push(write(&mut tx, rule, open.as_ref(), value, now, one).await?);
    }
    tx.commit().await?;
    Ok((true, written))
}

/// What the move from the alert's severity to `next` does, given its open
/// incident. With none open, a breach opens one — or reopens its last — and
/// so does a window empty for as long as the window, where the alert asks.
/// With one open, a recovery resolves it; a rise to `alert`, or a breach of
/// one opened on no data, is told again; a fall to `warning` follows it in
/// silence; and a severity held is told again once its re-notification is
/// due, while nobody has taken the incident.
fn moves(
    rule: &RuleRow,
    next: Severity,
    since: DateTime<Utc>,
    open: Option<&IncidentRow>,
    now: DateTime<Utc>,
) -> Vec<Told> {
    let move_to = |kind, severity| Told {
        incident_id: open.map_or_else(Uuid::new_v4, |incident| incident.id),
        kind,
        severity,
        heard_at: severity,
        again: false,
        reminder: false,
        silent: false,
    };
    let Some(incident) = open else {
        if next.is_breach() && !rule.severity.is_breach() {
            return vec![move_to(NotificationKind::IncidentOpened, next)];
        }
        let lasted = TimeDelta::from_std(rule.lookback.duration()).is_ok_and(|w| since + w <= now);
        let untold = rule.told_at.is_none_or(|at| at < since);
        if next == Severity::NoData
            && rule.missing_data == MissingData::NotifyNoData
            && lasted
            && untold
        {
            return vec![move_to(NotificationKind::IncidentOpened, Severity::NoData)];
        }
        return Vec::new();
    };
    if next == Severity::Ok {
        return vec![Told {
            heard_at: incident.severity,
            ..move_to(NotificationKind::IncidentResolved, Severity::Ok)
        }];
    }
    if next.is_breach() && next != incident.severity {
        let risen = next == Severity::Alert || incident.severity == Severity::NoData;
        return vec![Told {
            again: risen,
            silent: !risen,
            ..move_to(NotificationKind::IncidentOpened, next)
        }];
    }
    let due = rule.renotify_minutes.is_some_and(|minutes| {
        rule.told_at
            .is_none_or(|at| at + TimeDelta::minutes(i64::from(minutes)) <= now)
    });
    if next == incident.severity && incident.status == IncidentStatus::Open && due {
        return vec![Told {
            again: true,
            reminder: true,
            ..move_to(NotificationKind::IncidentOpened, next)
        }];
    }
    Vec::new()
}

/// One move written: an incident opened, reopened, followed or resolved. One
/// a member resolved since it was read moves no further, and is told nothing.
async fn write(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    rule: &RuleRow,
    open: Option<&IncidentRow>,
    value: Option<f64>,
    now: DateTime<Utc>,
    told: Told,
) -> Result<Told, TelmoniError> {
    let unmoved = |told: Told| Told {
        silent: true,
        ..told
    };
    match (told.kind, open) {
        (NotificationKind::IncidentResolved, Some(incident)) => {
            let moved = sqlx::query(
                "UPDATE telemetry.incidents
                    SET status = 'resolved', resolved_at = $2, value = $3, updated_at = $2
                  WHERE id = $1 AND status <> 'resolved'",
            )
            .bind(incident.id)
            .bind(now)
            .bind(value)
            .execute(tx.conn())
            .await?
            .rows_affected()
                > 0;
            let told = Told {
                again: incident.reopened > 0,
                ..told
            };
            Ok(if moved { told } else { unmoved(told) })
        }
        (_, Some(incident)) => {
            let moved = sqlx::query(
                "UPDATE telemetry.incidents SET severity = $2, value = $3, updated_at = $4
                  WHERE id = $1 AND status <> 'resolved'",
            )
            .bind(incident.id)
            .bind(told.severity)
            .bind(value)
            .bind(now)
            .execute(tx.conn())
            .await?
            .rows_affected()
                > 0;
            Ok(if moved { told } else { unmoved(told) })
        }
        (_, None) => {
            let reopened: Option<Uuid> = sqlx::query_scalar(
                "UPDATE telemetry.incidents
                    SET status = 'open', severity = $2, value = $3, resolved_at = NULL,
                        resolved_by = NULL, reopened = reopened + 1, updated_at = $4
                  WHERE id = (SELECT id FROM telemetry.incidents
                               WHERE rule_id = $1 AND status = 'resolved' AND resolved_at >= $5
                               ORDER BY resolved_at DESC LIMIT 1)
                 RETURNING id",
            )
            .bind(rule.id)
            .bind(told.severity)
            .bind(value)
            .bind(now)
            .bind(now - REOPEN_WITHIN)
            .fetch_optional(tx.conn())
            .await?;
            if let Some(id) = reopened {
                return Ok(Told {
                    incident_id: id,
                    again: true,
                    ..told
                });
            }
            sqlx::query(
                "INSERT INTO telemetry.incidents
                    (id, rule_id, project_id, organization_id, severity, value, opened_at,
                     updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $7)",
            )
            .bind(told.incident_id)
            .bind(rule.id)
            .bind(&rule.project_id)
            .bind(&rule.organization_id)
            .bind(told.severity)
            .bind(value)
            .bind(now)
            .execute(tx.conn())
            .await?;
            Ok(told)
        }
    }
}

/// The alert's open incident, acknowledged or not.
async fn open_of(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    rule_id: Uuid,
) -> sqlx::Result<Option<IncidentRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {INCIDENT_COLUMNS} FROM telemetry.incidents AS incident
           JOIN telemetry.rules AS rule ON rule.id = incident.rule_id
          WHERE incident.rule_id = $1 AND incident.status <> 'resolved'"
    )))
    .bind(rule_id)
    .fetch_optional(tx.conn())
    .await
}

/// Where one of the project's incident's notices went, kept on it after the
/// notice is told: the connections it was queued to, beside those its others
/// reached, and the run it showed, last among the [`RUNS_KEPT`] it keeps.
pub async fn told<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
    connections: &[Uuid],
    run: Option<Uuid>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE telemetry.incidents AS incident
            SET told_to = ARRAY(SELECT DISTINCT unnest(incident.told_to || $3::uuid[]) ORDER BY 1),
                runs = CASE WHEN $4::uuid IS NULL THEN incident.runs ELSE (
                    SELECT moved.runs[greatest(1, cardinality(moved.runs) - $5 + 1):]
                      FROM (SELECT array_remove(incident.runs, $4::uuid) || $4::uuid AS runs)
                           AS moved) END
          WHERE incident.project_id = $1 AND incident.id = $2",
    )
    .bind(project_id)
    .bind(id)
    .bind(connections)
    .bind(run)
    .bind(RUNS_KEPT)
    .execute(tx.conn())
    .await
    .map(|_| ())
}

/// The project's incidents whose notices showed `run_id`, the newest first.
pub async fn showing<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    run_id: Uuid,
    limit: i64,
) -> sqlx::Result<Vec<IncidentRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {INCIDENT_COLUMNS} FROM telemetry.incidents AS incident
           JOIN telemetry.rules AS rule ON rule.id = incident.rule_id
          WHERE incident.project_id = $1 AND incident.runs @> ARRAY[$2::uuid]
          ORDER BY incident.opened_at DESC, incident.id
          LIMIT $3"
    )))
    .bind(project_id)
    .bind(run_id)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// The incidents still open, acknowledged or not, on those of `projects` the
/// organization holds, the newest first.
pub async fn open_in<B: HasOrganization>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
    projects: &[ProjectId],
    limit: i64,
) -> sqlx::Result<Vec<IncidentRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {INCIDENT_COLUMNS} FROM telemetry.incidents AS incident
           JOIN telemetry.rules AS rule ON rule.id = incident.rule_id
          WHERE incident.organization_id = $1
            AND incident.project_id = ANY($2)
            AND incident.status <> 'resolved'
          ORDER BY incident.opened_at DESC, incident.id
          LIMIT $3"
    )))
    .bind(organization_id)
    .bind(projects)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// The project's incidents: those not resolved first, then the newest.
pub async fn incidents<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    limit: i64,
) -> sqlx::Result<Vec<IncidentRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {INCIDENT_COLUMNS} FROM telemetry.incidents AS incident
           JOIN telemetry.rules AS rule ON rule.id = incident.rule_id
          WHERE incident.project_id = $1
          ORDER BY incident.status = 'resolved', incident.opened_at DESC, incident.id
          LIMIT $2"
    )))
    .bind(project_id)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// One of the project's incidents, taken by `by` where it is open: answers
/// it as it stands after, none where the project has no such incident.
pub async fn acknowledge<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
    by: &UserId,
) -> sqlx::Result<Option<IncidentRow>> {
    sqlx::query(
        "UPDATE telemetry.incidents
            SET status = 'acknowledged', acknowledged_at = now(), acknowledged_by = $3,
                updated_at = now()
          WHERE project_id = $1 AND id = $2 AND status = 'open'",
    )
    .bind(project_id)
    .bind(id)
    .bind(by)
    .execute(tx.conn())
    .await?;
    one(tx, project_id, id).await
}

/// One of the project's incidents, resolved by `by` where it was not:
/// answers it as it stands after, and whether this resolved it.
pub async fn resolve<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
    by: &UserId,
) -> sqlx::Result<(Option<IncidentRow>, bool)> {
    let resolved = sqlx::query(
        "UPDATE telemetry.incidents
            SET status = 'resolved', resolved_at = now(), resolved_by = $3, updated_at = now()
          WHERE project_id = $1 AND id = $2 AND status <> 'resolved'",
    )
    .bind(project_id)
    .bind(id)
    .bind(by)
    .execute(tx.conn())
    .await?
    .rows_affected()
        > 0;
    Ok((one(tx, project_id, id).await?, resolved))
}

/// One of the project's incidents.
pub async fn one<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<Option<IncidentRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {INCIDENT_COLUMNS} FROM telemetry.incidents AS incident
           JOIN telemetry.rules AS rule ON rule.id = incident.rule_id
          WHERE incident.project_id = $1 AND incident.id = $2"
    )))
    .bind(project_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_crosses_the_alert_threshold_before_the_warning() {
        let cross = |v| crossed(RuleOperator::Gt, 5_000.0, Some(3_000.0), v);
        assert_eq!(cross(6_000.0), Severity::Alert);
        assert_eq!(cross(4_000.0), Severity::Warning);
        assert_eq!(cross(1_000.0), Severity::Ok);
        assert_eq!(
            crossed(RuleOperator::Lt, 1.0, None, 0.0),
            Severity::Alert,
            "an alert on too few"
        );
    }
}
