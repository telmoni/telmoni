//! The notices telemetry raises: an alert's incidents, told after the
//! evaluation that moved them commits, or when a member resolves one. An
//! opening and a resolution go to the project's feed and its connectors,
//! keyed by the incident and the kind, so an incident has one feed row of
//! each; what it says again goes to the connectors alone. A notice names the
//! alert and its numbers, never a span's content.

use serde_json::json;
use uuid::Uuid;

use chrono::Utc;

use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::seam::{Notice, Route};
use telmoni_shared::{
    NotificationKind, OrganizationId, ProjectId, RuleAggregation, RuleMeasure, Severity, SpanKind,
    TelmoniError,
};

use crate::AppState;
use crate::db::TelemetryLane;
use crate::rules::db::{self, RuleRow};
use crate::rules::incidents::Told;
use crate::rules::{Link, deliveries};

/// What an evaluation's moves say, told after its commit: each opening and
/// resolution emitted to the feed, keyed by the incident and the kind, and
/// said to the connectors alone where the feed holds it already or the move
/// is one said again. A notice that cannot be told is logged by its ids and
/// lost to the record, as the evaluation stands.
pub async fn tell_incidents(state: &AppState, rule: &RuleRow, value: Option<f64>, told: &[Told]) {
    if told.iter().all(|one| one.silent) {
        return;
    }
    let links = match links_of(state, rule).await {
        Ok(links) => links,
        Err(error) => {
            tracing::warn!(
                rule_id = %rule.id,
                error = %error,
                "telemetry: where an alert's notices go could not be read; the feed alone hears them"
            );
            Vec::new()
        }
    };
    let route_key = db::route_key(rule.id);
    for one in told.iter().filter(|one| !one.silent) {
        let now = Utc::now();
        let hearers: Vec<Uuid> = links
            .iter()
            .filter(|link| link.hears(one.heard_at, now))
            .map(|link| link.connection_id)
            .collect();
        let taken = deliveries::take(state, &rule.organization_id, hearers.len()).await;
        let route = Route {
            key: &route_key,
            connections: hearers.get(..taken).unwrap_or_default(),
        };
        let (title, body) = words(rule, value, one);
        let metadata = json!({
            "incident_id": one.incident_id,
            "rule_id": rule.id,
            "severity": one.severity,
            "value": value,
            "alert_threshold": rule.alert_threshold,
            "warning_threshold": rule.warning_threshold,
            "reminder": one.reminder,
        });
        let key = format!(
            "telemetry:incident:{}:{}",
            one.incident_id,
            if one.kind == NotificationKind::IncidentResolved {
                "resolved"
            } else {
                "opened"
            }
        );
        let notice = |dedup_key| Notice {
            kind: one.kind,
            subject_user_id: None,
            title: &title,
            body: &body,
            metadata: metadata.clone(),
            dedup_key,
        };
        let again = if one.again {
            true
        } else {
            match state
                .notifications
                .emit_routed(
                    &rule.organization_id,
                    &rule.project_id,
                    notice(Some(&key)),
                    route,
                )
                .await
            {
                Ok(emitted) => emitted.deduplicated,
                Err(error) => {
                    tracing::warn!(
                        rule_id = %rule.id,
                        incident_id = %one.incident_id,
                        error = %error,
                        "telemetry: an incident's notice could not be told"
                    );
                    false
                }
            }
        };
        if again
            && let Err(error) = state
                .notifications
                .remind(&rule.organization_id, &rule.project_id, notice(None), route)
                .await
        {
            tracing::warn!(
                rule_id = %rule.id,
                incident_id = %one.incident_id,
                error = %error,
                "telemetry: an incident's notice could not be told to the connectors"
            );
        }
    }
}

/// The connectors the alert links, read in the module's lane.
async fn links_of(state: &AppState, rule: &RuleRow) -> Result<Vec<Link>, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let links = db::links_of(&mut tx, rule.id).await?;
    tx.commit().await?;
    Ok(links.into_iter().map(db::LinkRow::link).collect())
}

/// A notice's title and body: the alert's name and where it stands, and its
/// metric in words with the value that set it — numbers alone, never a
/// span's content.
fn words(rule: &RuleRow, value: Option<f64>, told: &Told) -> (String, String) {
    let what = metric(rule);
    let shown = value.map_or_else(
        || "no value".to_owned(),
        |v| amount(rule.measure, rule.aggregation, v),
    );
    let threshold = |t: f64| amount(rule.measure, rule.aggregation, t);
    let (state, body) = match (told.kind, told.severity) {
        (NotificationKind::IncidentResolved, _) => (
            "resolved",
            format!("{what} is {shown}, inside its thresholds again."),
        ),
        (NotificationKind::IncidentOpened, Severity::NoData) => (
            "no data",
            format!("{what} has had no span to read for as long as its window."),
        ),
        (NotificationKind::IncidentOpened, Severity::Warning) => (
            "warning",
            format!(
                "{what} is {shown}, past the warning threshold of {}.",
                rule.warning_threshold.map_or_else(String::new, threshold)
            ),
        ),
        (_, _) => (
            "alert",
            format!(
                "{what} is {shown}, past the alert threshold of {}.",
                threshold(rule.alert_threshold)
            ),
        ),
    };
    let lead = if told.reminder { "Still: " } else { "" };
    (format!("{}: {state}", rule.name), format!("{lead}{body}"))
}

/// The alert's metric in words: "The p95 duration of model calls over the last hour".
fn metric(rule: &RuleRow) -> String {
    let folded = match (rule.measure, rule.aggregation) {
        (RuleMeasure::Count, _) => "The count".to_owned(),
        (RuleMeasure::Failures, RuleAggregation::Avg) => "The share failed".to_owned(),
        (RuleMeasure::Failures, _) => "The failures".to_owned(),
        (RuleMeasure::Duration, RuleAggregation::Avg) => "The average duration".to_owned(),
        (RuleMeasure::Duration, RuleAggregation::Sum) => "The summed duration".to_owned(),
        (RuleMeasure::Duration, percentile) => format!("The {percentile} duration"),
        (RuleMeasure::Tokens, RuleAggregation::Avg) => "The average tokens".to_owned(),
        (RuleMeasure::Tokens, _) => "The tokens".to_owned(),
    };
    let of = match rule.kind {
        SpanKind::Run => "runs",
        SpanKind::Step => "steps",
        SpanKind::ModelCall => "model calls",
        SpanKind::ToolCall => "tool calls",
        SpanKind::Span => "spans",
    };
    format!("{folded} of {of} over the last {}", rule.lookback)
}

/// A value as a notice writes it: durations in milliseconds, a share as a
/// percentage, the rest as numbers, to two places at most.
fn amount(measure: RuleMeasure, aggregation: RuleAggregation, value: f64) -> String {
    let number = |v: f64| {
        let text = format!("{v:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    };
    match (measure, aggregation) {
        (RuleMeasure::Duration, _) => format!("{} ms", number(value)),
        (RuleMeasure::Failures, RuleAggregation::Avg) => format!("{}%", number(value * 100.0)),
        _ => number(value),
    }
}

/// A member's resolution of an incident, told as a recovery is: in the feed,
/// keyed as the incident's resolution, and to the connectors its alert links
/// that heard it.
pub async fn resolved_by_hand(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    incident_id: Uuid,
    rule_id: Uuid,
    rule_name: &str,
    severity: Severity,
) {
    let title = format!("{rule_name}: resolved");
    let key = format!("telemetry:incident:{incident_id}:resolved");
    let notice = Notice {
        kind: NotificationKind::IncidentResolved,
        subject_user_id: None,
        title: &title,
        body: "A member of the project resolved this incident.",
        metadata: json!({
            "incident_id": incident_id,
            "rule_id": rule_id,
            "severity": severity,
            "by_hand": true,
        }),
        dedup_key: Some(&key),
    };
    let links = {
        let read = async {
            let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
            let links = db::links_of(&mut tx, rule_id).await?;
            tx.commit().await?;
            Ok::<_, TelmoniError>(links)
        };
        read.await.unwrap_or_default()
    };
    let now = Utc::now();
    let hearers: Vec<Uuid> = links
        .into_iter()
        .map(db::LinkRow::link)
        .filter(|link| link.hears(severity, now))
        .map(|link| link.connection_id)
        .collect();
    let taken = deliveries::take(state, organization_id, hearers.len()).await;
    let route_key = db::route_key(rule_id);
    let route = Route {
        key: &route_key,
        connections: hearers.get(..taken).unwrap_or_default(),
    };
    if let Err(error) = state
        .notifications
        .emit_routed(organization_id, project_id, notice, route)
        .await
    {
        tracing::warn!(
            project_id = %project_id,
            incident_id = %incident_id,
            error = %error,
            "telemetry: an incident resolved by hand could not be told"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_read_as_their_measure() {
        assert_eq!(
            amount(RuleMeasure::Duration, RuleAggregation::P95, 6_200.0),
            "6200 ms"
        );
        assert_eq!(
            amount(RuleMeasure::Failures, RuleAggregation::Avg, 0.25),
            "25%"
        );
        assert_eq!(
            amount(RuleMeasure::Tokens, RuleAggregation::Avg, 101.333),
            "101.33"
        );
    }
}
