//! A project's starter alerts: made once, when its first run lands, so a new
//! project is watched before anybody chooses a threshold — the share of its
//! runs that failed, a run looping on one tool, and a run left open — each
//! counted in the plan's alerts as any alert is. One past the plan's number
//! is not made, and the alerts page names it; one deleted is never made
//! again, since the settings row remembers the starters were made.

use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit, lock_chain_in_time};
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::{HasProject, Scoped, project_scope};
use telmoni_shared::seam;
use telmoni_shared::{
    AuditAction, MissingData, OrganizationId, ProjectId, RuleAggregation, RuleBaseline,
    RuleMeasure, RuleOperator, RuleWindow, SpanKind, TelemetryError, TelmoniError,
    TelmoniResourceKind,
};

use crate::AppState;
use crate::handler::chain;
use crate::handler::history::{self, HISTORY_DAYS};
use crate::rules::db::{self as rules, RuleRow};
use crate::rules::{RuleFilters, RuleInput};

/// Who files a starter's making in the audit chain.
pub const ACTOR: &str = "starter-alerts";

/// The failure share's starting numbers, the book's: an alert past 10% of
/// the last hour's runs, a warning past 5%.
pub const FAILURES_ALERT: f64 = 0.10;
pub const FAILURES_WARNING: f64 = 0.05;

/// The three, in the order they are made and withheld. A run's cost and
/// duration wait for its agent's own normal, so no run pages twice.
#[must_use]
pub fn starters() -> [RuleInput; 3] {
    let starter = |name: &str, measure, aggregation, operator, alert, warning| RuleInput {
        name: name.to_owned(),
        kind: SpanKind::Run,
        measure,
        aggregation,
        filters: RuleFilters::default(),
        window: RuleWindow::Hour,
        operator,
        alert_threshold: alert,
        warning_threshold: warning,
        missing_data: MissingData::Zero,
        renotify_minutes: None,
        score_name: None,
        baseline: RuleBaseline::None,
        objective: None,
        connectors: Vec::new(),
    };
    [
        starter(
            "Failing runs",
            RuleMeasure::Failures,
            RuleAggregation::Avg,
            RuleOperator::Gt,
            FAILURES_ALERT,
            Some(FAILURES_WARNING),
        ),
        starter(
            "Looping runs",
            RuleMeasure::Repeats,
            RuleAggregation::Max,
            RuleOperator::Gte,
            10.0,
            None,
        ),
        starter(
            "Stalled runs",
            RuleMeasure::Stalled,
            RuleAggregation::Count,
            RuleOperator::Gte,
            1.0,
            None,
        ),
    ]
}

/// The project's starters made unless they were, in a task of its own so an
/// export's answer never waits on them, and one a process at once; once a
/// process knows they were, it asks no more. One that fails is tried again
/// by the project's next export.
pub(crate) fn make_once(
    state: Arc<AppState>,
    organization_id: OrganizationId,
    project_id: ProjectId,
) {
    if state.starters_made.get(&project_id).is_some() {
        return;
    }
    state.starters_made.put(project_id.clone(), ());
    tokio::spawn(async move {
        if let Err(error) = make(&state, &organization_id, &project_id).await {
            state.starters_made.forget(&project_id);
            tracing::warn!(
                project_id = %project_id,
                error = %error,
                "telemetry: a project's starter alerts could not be made; its next export tries again"
            );
        }
    });
}

/// What one making came to: the starters made, by name, and those past the
/// plan's number, with the plan's sentence on them.
pub(crate) struct Made {
    pub(crate) names: Vec<String>,
    pub(crate) withheld: Vec<RuleMeasure>,
    pub(crate) detail: Option<String>,
}

/// The starters made in one transaction: claimed on the settings row first,
/// so they are made once whatever replicas race, then under the
/// organization's chain lock, which every alert's making takes, so two
/// projects' first runs never both take the plan's last place. Each is filed
/// as the service's, and the project's feed is told after the commit.
///
/// # Errors
///
/// Postgres refused, the lock was not had in time, or the project is moving.
pub async fn make(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
) -> Result<(), TelmoniError> {
    let ceilings = seam::alert_ceilings(state.entitlements.as_ref(), organization_id).await;
    let mut tx = project_scope(&state.db, project_id)
        .await?
        .bind_organization(organization_id)
        .await?;
    if !project_move::try_hold_in_place(tx.conn(), project_id).await? {
        chain::rolled_back(tx.rollback().await, project_id, "starters");
        return Err(TelemetryError::StoreUnavailable(
            "the project is moving to another organization, or being deleted".into(),
        )
        .into());
    }
    // None where they were made, or the project's row is gone with it.
    let claimed: Option<bool> = sqlx::query_scalar(
        "UPDATE telemetry.project_settings SET starters_made_at = now()
          WHERE project_id = $1 AND starters_made_at IS NULL RETURNING true",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?;
    if claimed.is_none() {
        tx.commit().await?;
        return Ok(());
    }
    lock_chain_in_time(tx.conn(), organization_id).await?;
    let mut held =
        u64::try_from(rules::organization_count(&mut tx, organization_id).await?).unwrap_or(0);
    // The project's own cap on its alerts, which a starter counts in too.
    let caps = crate::project_limits::caps_of(&mut tx, project_id).await?;
    let mut project_held =
        u64::try_from(rules::project_count(&mut tx, project_id, false).await?).unwrap_or(0);
    let taken: Vec<String> =
        sqlx::query_scalar("SELECT lower(name) FROM telemetry.rules WHERE project_id = $1")
            .bind(project_id)
            .fetch_all(tx.conn())
            .await?;
    let mut made = Made {
        names: Vec::new(),
        withheld: Vec::new(),
        detail: None,
    };
    for input in starters() {
        if let Some(ceilings) = &ceilings
            && ceilings.admit(held).is_err()
        {
            made.withheld.push(input.measure);
            made.detail = Some(ceilings.detail.clone());
            continue;
        }
        if let Err(refused) = crate::project_limits::admit(
            &caps,
            crate::project_limits::Counted::Alerts,
            project_held,
        ) {
            made.withheld.push(input.measure);
            made.detail = refused.to_problem_details().detail;
            continue;
        }
        let mut rule = input.checked()?;
        // A member's alert of the name came first.
        if taken.contains(&rule.definition.name.to_lowercase()) {
            rule.definition.name = format!("{} (starter)", rule.definition.name);
        }
        let row = rules::insert(
            &mut tx,
            project_id,
            organization_id,
            (Uuid::new_v4(), true),
            &rule,
        )
        .await?;
        let id = row.id.to_string();
        emit_audit(
            &mut tx,
            AuditEvent::new(
                organization_id,
                Actor::Service(ACTOR),
                AuditAction::Created,
                TelmoniResourceKind::Alert,
            )
            .in_project(project_id)
            .on(&id)
            .details(json!({ "name": row.definition.name, "starter": true })),
        )
        .await?;
        held += 1;
        project_held += 1;
        made.names.push(row.definition.name);
    }
    if !made.withheld.is_empty() {
        let withheld: Vec<String> = made.withheld.iter().map(ToString::to_string).collect();
        sqlx::query(
            "UPDATE telemetry.project_settings SET starters_withheld = $2 WHERE project_id = $1",
        )
        .bind(project_id)
        .bind(&withheld)
        .execute(tx.conn())
        .await?;
    }
    tx.commit().await?;
    tracing::info!(
        project_id = %project_id,
        made = made.names.len(),
        withheld = made.withheld.len(),
        "telemetry: a project's starter alerts made"
    );
    crate::notify::starters_made(state, organization_id, project_id, &made).await;
    Ok(())
}

/// A starter's name, by its measure.
pub(crate) fn name_of(measure: RuleMeasure) -> String {
    starters()
        .into_iter()
        .find(|starter| starter.measure == measure)
        .map_or_else(|| measure.to_string(), |starter| starter.name)
}

/// Names in a sentence: `a`, `a and b`, `a, b and c`.
pub(crate) fn words(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The starters its plan had no room for, by measure.
///
/// # Errors
///
/// Postgres refused.
pub async fn withheld<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Vec<RuleMeasure>> {
    let held: Option<Vec<String>> = sqlx::query_scalar(
        "SELECT starters_withheld FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?;
    Ok(held
        .unwrap_or_default()
        .iter()
        .filter_map(|measure| measure.parse().ok())
        .collect())
}

/// A share as a threshold is typed: whole percents, up, and at least one.
fn whole_percent(share: f64) -> f64 {
    // Less a hair first, so a share of 0.2, 20.000000000000004 percent, stays 20.
    (share * 100.0 - 1e-9).ceil().max(1.0) / 100.0
}

/// `rule` as the pass evaluates it: the failure share's starter, once the
/// project has run fourteen days, set once to the suggestion an alert's form
/// offers from its history — the 99th percentile of its hours for the alert,
/// the 90th for the warning — unless a member settled it first. A project
/// with no run in those days keeps the starting number. One that fails is
/// logged and tried again by the next pass.
pub async fn tuned(state: &AppState, rule: RuleRow, now: DateTime<Utc>) -> RuleRow {
    let due = rule.starter
        && rule.tuned_at.is_none()
        && rule.definition.kind == SpanKind::Run
        && rule.definition.measure == RuleMeasure::Failures
        && rule.definition.aggregation == RuleAggregation::Avg
        && rule.created_at <= now - TimeDelta::days(HISTORY_DAYS);
    if !due {
        return rule;
    }
    match tune(state, &rule, now).await {
        Ok(tuned) => tuned.unwrap_or(rule),
        Err(error) => {
            tracing::warn!(
                project_id = %rule.project_id,
                rule_id = %rule.id,
                error = %error,
                "telemetry: a starter's threshold could not be set from its history; the next pass tries again"
            );
            rule
        }
    }
}

/// The tune itself, filed as the service's: the row as written, none where
/// a member's edit came first.
async fn tune(
    state: &AppState,
    rule: &RuleRow,
    now: DateTime<Utc>,
) -> Result<Option<RuleRow>, TelmoniError> {
    let history = history::read(state, &rule.project_id, now).await?;
    let share = history
        .agents
        .iter()
        .find(|row| row.agent.is_none() && row.kind == SpanKind::Run)
        .map(|runs| runs.failure_share);
    let (alert, warning) = match share.and_then(|share| share.p99.map(|p99| (p99, share.p90))) {
        Some((p99, p90)) => {
            let alert = whole_percent(p99);
            (
                alert,
                p90.map(whole_percent).filter(|warning| *warning < alert),
            )
        }
        None => (
            rule.definition.alert_threshold,
            rule.definition.warning_threshold,
        ),
    };
    let organization_id = &rule.organization_id;
    let mut tx = project_scope(&state.db, &rule.project_id)
        .await?
        .bind_organization(organization_id)
        .await?;
    lock_chain_in_time(tx.conn(), organization_id).await?;
    let written: Option<DateTime<Utc>> = sqlx::query_scalar(
        "UPDATE telemetry.rules
            SET alert_threshold = $3, warning_threshold = $4, tuned_at = now()
          WHERE project_id = $1 AND id = $2 AND tuned_at IS NULL
         RETURNING tuned_at",
    )
    .bind(&rule.project_id)
    .bind(rule.id)
    .bind(alert)
    .bind(warning)
    .fetch_optional(tx.conn())
    .await?;
    let Some(tuned_at) = written else {
        tx.commit().await?;
        return Ok(None);
    };
    let changed: Vec<&str> = [
        (
            "alert_threshold",
            alert.to_bits() != rule.definition.alert_threshold.to_bits(),
        ),
        (
            "warning_threshold",
            warning.map(f64::to_bits) != rule.definition.warning_threshold.map(f64::to_bits),
        ),
    ]
    .into_iter()
    .filter_map(|(field, moved)| moved.then_some(field))
    .collect();
    if changed.is_empty() {
        tx.commit().await?;
        let mut kept = rule.clone();
        kept.tuned_at = Some(tuned_at);
        return Ok(Some(kept));
    }
    let id = rule.id.to_string();
    emit_audit(
        &mut tx,
        AuditEvent::new(
            organization_id,
            Actor::Service(ACTOR),
            AuditAction::Updated,
            TelmoniResourceKind::Alert,
        )
        .in_project(&rule.project_id)
        .on(&id)
        .details(json!({ "name": rule.definition.name, "changed": changed, "from": "history" })),
    )
    .await?;
    tx.commit().await?;
    tracing::info!(
        project_id = %rule.project_id,
        rule_id = %rule.id,
        "telemetry: a starter's threshold set from its project's history"
    );
    let mut tuned = rule.clone();
    tuned.definition.alert_threshold = alert;
    tuned.definition.warning_threshold = warning;
    tuned.tuned_at = Some(tuned_at);
    Ok(Some(tuned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_starters_pass_the_lanes_own_check() {
        for starter in starters() {
            let name = starter.name.clone();
            assert!(
                starter.checked().is_ok(),
                "{name} is refused by the lane's check"
            );
        }
    }

    #[test]
    fn a_share_is_set_in_whole_percents_up_and_never_below_one() {
        assert!((whole_percent(0.0) - 0.01).abs() < 1e-12);
        assert!((whole_percent(0.034) - 0.04).abs() < 1e-12);
        assert!((whole_percent(0.2) - 0.2).abs() < 1e-12);
    }

    #[test]
    fn names_read_as_a_sentence() {
        let names =
            |given: &[&str]| words(&given.iter().map(|&n| n.to_owned()).collect::<Vec<_>>());
        assert_eq!(names(&["Failing runs"]), "Failing runs");
        assert_eq!(names(&["A", "B"]), "A and B");
        assert_eq!(names(&["A", "B", "C"]), "A, B and C");
        assert_eq!(name_of(RuleMeasure::Stalled), "Stalled runs");
    }
}
