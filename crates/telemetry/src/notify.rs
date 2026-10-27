//! The notices telemetry raises: an alert's incidents, told after the
//! evaluation that moved them commits, or when a member resolves one. An
//! opening and a resolution go to the project's feed and its connectors,
//! keyed by the incident and the kind, so an incident has one feed row of
//! each; what it says again goes to the connectors alone. A notice names the
//! alert, where it stands and the numbers that crossed it, the run that shows
//! it and the traces it read: names and numbers alone, never a span's
//! content, since every connector is a third party.

use serde_json::json;
use uuid::Uuid;

use chrono::Utc;

use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::seam::{Notice, Route};
use telmoni_shared::{
    NotificationKind, OrganizationId, ProjectId, RuleAggregation, RuleMeasure, RuleWindow,
    Severity, SpanKind, SpanStatus, TelmoniError,
};

use crate::AppState;
use crate::db::TelemetryLane;
use crate::rules::db::{self, RuleRow};
use crate::rules::example::{self, ExampleRun, Pick};
use crate::rules::incidents::{self, IncidentRow, Told};
use crate::rules::{Link, deliveries, mutes};
use crate::spend::Unread;

/// What an evaluation's moves say, told after its commit: each opening and
/// resolution emitted to the feed, keyed by the incident and the kind, and
/// said to the connectors alone where the move is one said again. A notice
/// that cannot be told is logged by its ids and lost to the record.
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
    let muted = muted_now(state, &rule.project_id, rule.id).await;
    let run = run_of(state, rule, told).await;
    let traces_url = traces_link(state, rule);
    for one in told.iter().filter(|one| !one.silent) {
        let now = Utc::now();
        let hearers: Vec<Uuid> = links
            .iter()
            .filter(|link| !muted && link.hears(one.heard_at, now))
            .map(|link| link.connection_id)
            .collect();
        let taken = deliveries::take(state, &rule.organization_id, hearers.len()).await;
        let route = Route {
            key: &route_key,
            connections: &hearers,
            most: taken.room(),
        };
        let (title, body) = words(rule, value, one);
        let shown = run.as_ref().filter(|_| shows_run(one));
        let body = carrying(
            body,
            [
                shown.map(|run| run_line(state, rule, run)),
                traces_url.as_ref().map(|url| format!("Traces: {url}")),
            ],
        );
        let metadata = json!({
            "incident_id": one.incident_id,
            "rule_id": rule.id,
            "severity": one.severity,
            "value": value,
            "alert_threshold": rule.alert_threshold,
            "warning_threshold": rule.warning_threshold,
            "reminder": one.reminder,
            "run": shown.map(|run| run_json(state, rule, run)),
            "traces_url": traces_url,
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
        let reached = tell(
            state,
            &rule.organization_id,
            &rule.project_id,
            notice,
            &key,
            route,
            one.again,
        )
        .await;
        let made = reached
            .as_ref()
            .map_or(0, |reached| reached.connections.len());
        deliveries::settle(state, &rule.organization_id, taken, made).await;
        match reached {
            Ok(reached) => {
                let run = shown.filter(|_| reached.anyone).map(|run| run.run_id);
                keep_told(
                    state,
                    &rule.project_id,
                    one.incident_id,
                    &reached.connections,
                    run,
                )
                .await;
            }
            Err(error) => tracing::warn!(
                rule_id = %rule.id,
                incident_id = %one.incident_id,
                error = %error,
                "telemetry: an incident's notice could not be told"
            ),
        }
    }
}

/// Where a notice reached: the connections it was queued to, and whether
/// anyone heard it, a feed row of its own or a connector.
struct Reached {
    connections: Vec<Uuid>,
    anyone: bool,
}

/// A notice of an incident told to the feed under `key` and to its route, or
/// to the route alone where it is said `again`. A replay of the key tells
/// nobody, its notice told once already.
async fn tell<'a>(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    notice: impl Fn(Option<&'a str>) -> Notice<'a>,
    key: &'a str,
    route: Route<'_>,
    again: bool,
) -> Result<Reached, TelmoniError> {
    if again {
        let connections = state
            .notifications
            .remind(organization_id, project_id, notice(None), route)
            .await?;
        return Ok(Reached {
            anyone: !connections.is_empty(),
            connections,
        });
    }
    let routed = state
        .notifications
        .emit_routed(organization_id, project_id, notice(Some(key)), route)
        .await?;
    Ok(Reached {
        anyone: !routed.emitted.deduplicated,
        connections: routed.connections,
    })
}

/// Where one of an incident's notices went, and the run it showed, kept on
/// the incident in its project's own scope; logged by its ids where it cannot
/// be, as the notice stands.
async fn keep_told(
    state: &AppState,
    project_id: &ProjectId,
    incident_id: Uuid,
    connections: &[Uuid],
    run: Option<Uuid>,
) {
    if connections.is_empty() && run.is_none() {
        return;
    }
    let write = async {
        let mut tx = project_scope(&state.db, project_id).await?;
        incidents::told(&mut tx, project_id, incident_id, connections, run).await?;
        tx.commit().await?;
        Ok::<_, TelmoniError>(())
    };
    if let Err(error) = write.await {
        tracing::warn!(
            incident_id = %incident_id,
            error = %error,
            "telemetry: where an incident's notice went could not be kept"
        );
    }
}

/// Whether a maintenance window mutes the alert now; none, logged, where
/// the windows cannot be read, so a notice is told rather than lost.
async fn muted_now(state: &AppState, project_id: &ProjectId, rule_id: Uuid) -> bool {
    let read = async {
        let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
        let muted = mutes::in_force(&mut tx, project_id, rule_id, Utc::now()).await?;
        tx.commit().await?;
        Ok::<_, TelmoniError>(muted)
    };
    match read.await {
        Ok(muted) => muted,
        Err(error) => {
            tracing::warn!(
                rule_id = %rule_id,
                error = %error,
                "telemetry: an alert's maintenance windows could not be read; it is told"
            );
            false
        }
    }
}

/// A notice's body, then a line for each of what else it carries.
fn carrying<const N: usize>(body: String, lines: [Option<String>; N]) -> String {
    std::iter::once(body)
        .chain(lines.into_iter().flatten())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a move shows the run: a breach opened, reopened or risen, which
/// a window with no span cannot show; a recovery need not, nor a reminder,
/// which may come every minute and whose opening named one.
fn shows_run(one: &Told) -> bool {
    one.kind == NotificationKind::IncidentOpened
        && one.severity != Severity::NoData
        && !one.reminder
}

/// The run an evaluation's breaches show, read once; none, logged by the
/// error's kind, where a store does not answer, and the notice goes on.
async fn run_of(state: &AppState, rule: &RuleRow, told: &[Told]) -> Option<ExampleRun> {
    if !told.iter().any(|one| !one.silent && shows_run(one)) {
        return None;
    }
    match example::example(state, rule, Utc::now()).await {
        Ok(run) => run,
        Err(Unread::Store(error)) => {
            tracing::warn!(
                rule_id = %rule.id,
                kind = crate::store::error_kind(&error),
                "telemetry: the run an alert's notice shows could not be read; it goes without"
            );
            None
        }
        Err(Unread::Db(error)) => {
            tracing::warn!(
                rule_id = %rule.id,
                error = %error,
                "telemetry: the prices of the run an alert's notice shows could not be read; \
                 it goes without"
            );
            None
        }
    }
}

/// The console's trace list as the alert read it, spelled with ids: its
/// filters, its window in days, and where it counts failures, those that
/// failed. The list has no filter by provider, so an alert's widens it; none
/// past [`LINK_MAX`], whose filters would run the notice past its bounds.
fn traces_link(state: &AppState, rule: &RuleRow) -> Option<String> {
    let origin = state.config.app_url.as_deref()?;
    let mut url = reqwest::Url::parse(&format!(
        "{origin}/{}/{}/traces",
        rule.organization_id, rule.project_id
    ))
    .ok()?;
    let filters = &rule.filters.0;
    let failed = vec![SpanStatus::Error];
    let statuses = if filters.status.is_empty() && rule.measure == RuleMeasure::Failures {
        &failed
    } else {
        &filters.status
    };
    {
        let mut query = url.query_pairs_mut();
        query.append_pair(
            "days",
            match rule.lookback {
                RuleWindow::Hour | RuleWindow::Day => "1",
                RuleWindow::Week => "7",
            },
        );
        for (key, values) in [
            ("agent", &filters.agent),
            ("model", &filters.model),
            ("tool", &filters.tool),
            ("environment", &filters.environment),
            ("release", &filters.release),
        ] {
            for value in values {
                query.append_pair(key, value);
            }
        }
        for status in statuses {
            query.append_pair("status", &status.to_string());
        }
    }
    let url: String = url.into();
    (url.len() <= LINK_MAX).then_some(url)
}

/// The longest trace list link a notice carries, in bytes.
const LINK_MAX: usize = 1_024;

/// A run's own page in the console, spelled with ids.
fn run_link(state: &AppState, rule: &RuleRow, run_id: Uuid) -> Option<String> {
    let origin = state.config.app_url.as_deref()?;
    Some(format!(
        "{origin}/{}/{}/traces/{run_id}",
        rule.organization_id, rule.project_id
    ))
}

/// The line naming the run a notice shows: which it is, its last step, its
/// error, its steps, its tokens and what it spent, the models it called that
/// no price covers, and where it opens.
fn run_line(state: &AppState, rule: &RuleRow, run: &ExampleRun) -> String {
    let label = picked(rule.kind, run.pick);
    let mut parts = Vec::new();
    if let Some(step) = &run.last_step {
        parts.push(format!("last step {}", clipped(step)));
    }
    if let Some(error) = &run.error_type {
        parts.push(clipped(error));
    }
    parts.push(format!(
        "{} {}",
        run.steps,
        if run.steps == 1 { "step" } else { "steps" }
    ));
    parts.push(format!(
        "{} tokens and {}{}",
        run.tokens,
        dollars(run.spent),
        if run.ended { "" } else { " so far" }
    ));
    if !run.unpriced.is_empty() {
        parts.push(format!("no price for {}", unpriced_names(&run.unpriced)));
    }
    let details = parts.join(", ");
    match run_link(state, rule, run.run_id) {
        Some(url) => format!("{label}: {details}. {url}"),
        None => format!("{label} ({}): {details}.", run.run_id),
    }
}

/// The most unpriced names a run's line carries, the rest counted.
const UNPRICED_SHOWN: usize = 3;

/// A run's unpriced names in words: `gpt-x (openai), ft-model (openai) and 2 more`.
fn unpriced_names(names: &[(String, String)]) -> String {
    let shown: Vec<String> = names
        .iter()
        .take(UNPRICED_SHOWN)
        .map(
            |(provider, model)| match (provider.is_empty(), model.is_empty()) {
                (_, true) => "a call naming no model".to_owned(),
                (true, false) => clipped(model),
                (false, false) => format!("{} ({})", clipped(model), clipped(provider)),
            },
        )
        .collect();
    let rest = names.len().saturating_sub(UNPRICED_SHOWN);
    if rest == 0 {
        shown.join(", ")
    } else {
        format!("{} and {rest} more", shown.join(", "))
    }
}

/// The run a notice shows, as its metadata carries it: names clipped and the
/// unpriced models counted, so it stays within what the feed keeps.
fn run_json(state: &AppState, rule: &RuleRow, run: &ExampleRun) -> serde_json::Value {
    json!({
        "run_id": run.run_id,
        "url": run_link(state, rule, run.run_id),
        "last_step": run.last_step.as_deref().map(clipped),
        "error_type": run.error_type.as_deref().map(clipped),
        "steps": run.steps,
        "tokens": run.tokens,
        "spent": run.spent,
        "unpriced": run.unpriced.len(),
        "ended": run.ended,
    })
}

/// Which run a notice shows, in words.
fn picked(kind: SpanKind, pick: Pick) -> String {
    let one = match kind {
        SpanKind::Run => "run",
        SpanKind::Step => "step",
        SpanKind::ModelCall => "model call",
        SpanKind::ToolCall => "tool call",
        SpanKind::Span => "span",
    };
    match (pick, kind) {
        (Pick::LatestFailed, SpanKind::Run) => "The latest failed run".to_owned(),
        (Pick::LatestFailed, _) => format!("The run of the latest failed {one}"),
        (Pick::Slowest, SpanKind::Run) => "The slowest run".to_owned(),
        (Pick::Slowest, _) => format!("The run of the slowest {one}"),
        (Pick::MostTokens, SpanKind::Run) => "The run with the most tokens".to_owned(),
        (Pick::MostTokens, _) => format!("The run of the {one} with the most tokens"),
        (Pick::MostSpent, _) => "The run that spent the most".to_owned(),
        (Pick::Latest, SpanKind::Run) => "The latest run".to_owned(),
        (Pick::Latest, _) => format!("The run of the latest {one}"),
    }
}

/// A name as a notice carries it: one line, at most [`NAME_SHOWN`] characters.
fn clipped(name: &str) -> String {
    let line: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if line.chars().count() <= NAME_SHOWN {
        return line;
    }
    let mut shown: String = line.chars().take(NAME_SHOWN - 1).collect();
    shown.push('…');
    shown
}

/// The most of a step's name or an error's type a notice carries.
const NAME_SHOWN: usize = 120;

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

/// The alert's metric in words: "The p95 duration of model calls over the
/// last hour", "The most a run spent over the last day".
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
        (RuleMeasure::Cost, aggregation) => {
            let spent = match aggregation {
                RuleAggregation::Max => "The most a run spent",
                RuleAggregation::Avg => "The average a run spent",
                _ => "What the runs spent",
            };
            return format!("{spent} over the last {}", rule.lookback);
        }
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
/// percentage, a cost in dollars, the rest as numbers, to two places at most.
fn amount(measure: RuleMeasure, aggregation: RuleAggregation, value: f64) -> String {
    let number = |v: f64| {
        let text = format!("{v:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    };
    match (measure, aggregation) {
        (RuleMeasure::Duration, _) => format!("{} ms", number(value)),
        (RuleMeasure::Failures, RuleAggregation::Avg) => format!("{}%", number(value * 100.0)),
        (RuleMeasure::Cost, _) => dollars(value),
        _ => number(value),
    }
}

/// US dollars as a notice writes them: to the cent from a dollar, else to
/// three figures, since a run's is often a fraction of a cent (`$5.20`,
/// `$0.0123`).
fn dollars(value: f64) -> String {
    let cents = format!("${value:.2}");
    if !(value.is_finite() && value > 0.0) {
        return cents;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a finite f64's tens are a few hundred at most"
    )]
    let places = usize::try_from(2 - value.log10().floor() as i64).unwrap_or(0);
    let figures = format!("{value:.places$}");
    // Rounded first, so $0.9999 reads as $1.00.
    if figures.parse::<f64>().is_ok_and(|shown| shown >= 1.0) {
        return cents;
    }
    // Two figures at least: $0.50, not $0.5.
    let (whole, fraction) = figures.split_once('.').unwrap_or((&figures, ""));
    let kept = fraction
        .trim_end_matches('0')
        .len()
        .max(places.saturating_sub(1));
    let fraction: String = fraction.chars().take(kept).collect();
    format!("${whole}.{fraction}")
}

/// A member's resolution of an incident, told as a recovery is: in the feed,
/// keyed as the incident's resolution, and to the connectors its alert links
/// that hear it; to them alone once it reopened, as the feed holds its first.
pub async fn resolved_by_hand(state: &AppState, incident: &IncidentRow) {
    let (organization_id, project_id) = (&incident.organization_id, &incident.project_id);
    let (incident_id, rule_id, severity) = (incident.id, incident.rule_id, incident.severity);
    let title = format!("{}: resolved", incident.rule_name);
    let key = format!("telemetry:incident:{incident_id}:resolved");
    let metadata = json!({
        "incident_id": incident_id,
        "rule_id": rule_id,
        "severity": severity,
        "by_hand": true,
    });
    let notice = |dedup_key| Notice {
        kind: NotificationKind::IncidentResolved,
        subject_user_id: None,
        title: &title,
        body: "A member of the project resolved this incident.",
        metadata: metadata.clone(),
        dedup_key,
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
    let muted = muted_now(state, project_id, rule_id).await;
    let hearers: Vec<Uuid> = links
        .into_iter()
        .map(db::LinkRow::link)
        .filter(|link| !muted && link.hears(severity, now))
        .map(|link| link.connection_id)
        .collect();
    let taken = deliveries::take(state, organization_id, hearers.len()).await;
    let route_key = db::route_key(rule_id);
    let route = Route {
        key: &route_key,
        connections: &hearers,
        most: taken.room(),
    };
    let reached = tell(
        state,
        organization_id,
        project_id,
        notice,
        &key,
        route,
        incident.reopened > 0,
    )
    .await;
    let made = reached
        .as_ref()
        .map_or(0, |reached| reached.connections.len());
    deliveries::settle(state, organization_id, taken, made).await;
    match reached {
        Ok(reached) => keep_told(state, project_id, incident_id, &reached.connections, None).await,
        Err(error) => tracing::warn!(
            project_id = %project_id,
            incident_id = %incident_id,
            error = %error,
            "telemetry: an incident resolved by hand could not be told"
        ),
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
        assert_eq!(
            amount(RuleMeasure::Cost, RuleAggregation::Max, 5.0),
            "$5.00"
        );
    }

    /// Dollars to the cent from one, a fraction of one to three figures, two at least.
    #[test]
    fn a_cost_reads_in_dollars() {
        for (value, shown) in [
            (0.0, "$0.00"),
            (5.2, "$5.20"),
            (1_234.5, "$1234.50"),
            (0.5, "$0.50"),
            (0.0123, "$0.0123"),
            (0.000_045, "$0.000045"),
            (0.999, "$0.999"),
            (0.9999, "$1.00"),
        ] {
            assert_eq!(dollars(value), shown, "{value}");
        }
    }

    /// A run's unpriced names are named, a few of them, and the rest counted.
    #[test]
    fn a_runs_unpriced_names_are_named_and_the_rest_counted() {
        let name = |provider: &str, model: &str| (provider.to_owned(), model.to_owned());
        assert_eq!(
            unpriced_names(&[name("openai", "ft:gpt-4o:acme")]),
            "ft:gpt-4o:acme (openai)"
        );
        assert_eq!(
            unpriced_names(&[name("", "llama3.1:8b"), name("", "")]),
            "llama3.1:8b, a call naming no model"
        );
        let many: Vec<_> = (1..=5).map(|n| name("vllm", &format!("m{n}"))).collect();
        assert_eq!(
            unpriced_names(&many),
            "m1 (vllm), m2 (vllm), m3 (vllm) and 2 more"
        );
    }

    /// A run is named for the span it was picked by, in the alert's kind.
    #[test]
    fn a_run_is_named_for_its_pick() {
        assert_eq!(
            picked(SpanKind::Run, Pick::LatestFailed),
            "The latest failed run"
        );
        assert_eq!(
            picked(SpanKind::ToolCall, Pick::LatestFailed),
            "The run of the latest failed tool call"
        );
        assert_eq!(
            picked(SpanKind::ModelCall, Pick::MostTokens),
            "The run of the model call with the most tokens"
        );
        assert_eq!(
            picked(SpanKind::Run, Pick::MostSpent),
            "The run that spent the most"
        );
    }

    /// A name keeps to one line and to its length, cut where it runs past.
    #[test]
    fn a_name_keeps_to_one_line_and_its_length() {
        assert_eq!(clipped("search\nthe web"), "search the web");
        let long = "x".repeat(NAME_SHOWN + 5);
        let shown = clipped(&long);
        assert_eq!(shown.chars().count(), NAME_SHOWN);
        assert!(shown.ends_with('…'));
        assert_eq!(clipped(&"y".repeat(NAME_SHOWN)), "y".repeat(NAME_SHOWN));
    }
}
