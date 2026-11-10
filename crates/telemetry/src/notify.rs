//! The notices telemetry raises: a project's starter alerts, told once its
//! first trace made them; an alert's incidents, told after the
//! evaluation that moved them commits, or when a member resolves one. An
//! opening and a resolution go to the project's feed and its connectors,
//! keyed by the incident and the kind, so an incident has one feed row of
//! each; what it says again goes to the connectors alone. A notice names the
//! alert, where it stands and the numbers that crossed it, the trace that
//! shows it, what its agents changed to in the day before it opened
//! ([`crate::changes`]) and the traces it read: names and numbers alone,
//! never a span's content, since every connector is a third party.

use std::time::Duration;

use serde_json::json;
use uuid::Uuid;

use chrono::{DateTime, TimeDelta, Utc};

use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::seam::{IncidentNotice, Notice, Route};
use telmoni_shared::{
    ChangeKind, IncidentStatus, MonitorCause, NotificationKind, OrganizationId, ProjectId,
    RuleAggregation, RuleBaseline, RuleMeasure, RuleOperator, RuleWindow, Severity, SpanKind,
    SpanStatus, TelmoniError,
};

use crate::AppState;
use crate::changes::{self, BEFORE_INCIDENT, Change, Opened, Scope};
use crate::db::TelemetryLane;
use crate::monitors::{Judged, MonitorRow};
use crate::rules::db::{self, RuleRow};
use crate::rules::example::{self, ExampleTrace, Pick, Repeated};
use crate::rules::incidents::{self, IncidentRow, Move, Owner, Telling, Told};
use crate::rules::{Link, deliveries, mutes};
use crate::spend::Unread;
use crate::traces::Facet;

/// What an evaluation's moves say, told after its commit: each opening and
/// resolution emitted to the feed, keyed by the incident and the kind, and
/// said to the connectors alone where the move is one said again. Windows and
/// hours are read at `at`, the instant the alert keeps as its last notice's.
/// A notice that cannot be told is logged by its ids and lost to the record.
pub async fn tell_incidents(
    state: &AppState,
    rule: &RuleRow,
    value: Option<f64>,
    told: &[Told],
    at: DateTime<Utc>,
) {
    if told.iter().all(|one| one.telling == Telling::Silent) {
        return;
    }
    let owner = Owner::Rule(rule.id);
    let links = match links_of(state, owner).await {
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
    let muted = muted_at(state, &rule.project_id, owner, at).await;
    let trace = trace_of(state, rule, told).await;
    let scope = Scope::of_alert(&rule.definition.filters);
    let changed = changed_of(state, &rule.project_id, &scope, told).await;
    for one in told.iter().filter(|one| one.telling != Telling::Silent) {
        // A resolution's `told_to` names who heard its incident; any other's is empty.
        let hearers: Vec<Uuid> = links
            .iter()
            .filter(|link| link.hears_end(one.heard_at, &one.told_to, muted, at))
            .map(|link| link.connection_id)
            .collect();
        let shown = Shown {
            trace: trace.as_ref().filter(|_| shows_trace(one)),
            changed: changed.as_ref().filter(|_| shows_trace(one)),
        };
        tell_one(state, rule, value, one, &hearers, shown).await;
    }
}

/// The alert's open incident told again, as a reminder, to the linked
/// connectors that hear it now but its last notice did not reach, a window or
/// their hours then having kept it from them (`told_to` names whom it
/// reached, so an edit of their hours or a window dated back tells nobody
/// twice); that notice is then now, so each is told it once. One taken is not.
pub async fn tell_held(state: &AppState, rule: &RuleRow, value: Option<f64>) {
    let Some(last) = rule.told_at else {
        return;
    };
    let now = Utc::now();
    let held = async {
        let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
        let open = incidents::open_of(&mut tx, Owner::Rule(rule.id)).await?;
        let Some(incident) = open.filter(|incident| incident.status == IncidentStatus::Open) else {
            return Ok(None);
        };
        let links = db::links_of(&mut tx, rule.id).await?;
        let muted = mutes::in_force(&mut tx, &rule.project_id, Owner::Rule(rule.id), now).await?;
        let hearers: Vec<Uuid> = links
            .into_iter()
            .map(db::LinkRow::link)
            .filter(|link| {
                !rule.told_to.contains(&link.connection_id)
                    && !muted
                    && link.hears(incident.severity, now)
            })
            .map(|link| link.connection_id)
            .collect();
        if hearers.is_empty() {
            return Ok(None);
        }
        let moved =
            sqlx::query("UPDATE telemetry.rules SET told_at = $3 WHERE id = $1 AND told_at = $2")
                .bind(rule.id)
                .bind(last)
                .bind(now)
                .execute(tx.conn())
                .await?
                .rows_affected()
                > 0;
        tx.commit().await?;
        Ok::<_, TelmoniError>(moved.then_some((incident, hearers)))
    };
    let (incident, hearers) = match held.await {
        Ok(Some(held)) => held,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(
                rule_id = %rule.id,
                error = %error,
                "telemetry: whether a hold kept an incident from its connectors could not be read"
            );
            return;
        }
    };
    let one = Told {
        incident_id: incident.id,
        change: Move::Opened,
        severity: incident.severity,
        heard_at: incident.severity,
        telling: Telling::Reminder,
        told_to: Vec::new(),
    };
    tell_one(state, rule, value, &one, &hearers, Shown::default()).await;
}

/// What a breach's notice shows beside its words: the trace, and what its
/// agents changed to before it opened.
#[derive(Clone, Copy, Default)]
struct Shown<'a> {
    trace: Option<&'a ExampleTrace>,
    changed: Option<&'a Changed>,
}

/// One move told to `hearers`, and to the feed unless said again, with what
/// it shows; the day's deliveries taken, and where it went kept on its incident.
async fn tell_one(
    state: &AppState,
    rule: &RuleRow,
    value: Option<f64>,
    one: &Told,
    hearers: &[Uuid],
    Shown {
        trace: shown,
        changed,
    }: Shown<'_>,
) {
    let traces_url = traces_link(state, rule);
    let (title, body) = words(rule, value, one);
    let body = carrying(
        body,
        [
            shown.map(|trace| trace_line(state, rule, trace, one.severity)),
            traces_url.as_ref().map(|url| format!("Traces: {url}")),
        ],
        changed,
    );
    let metadata = json!({
        "incident_id": one.incident_id,
        "rule_id": rule.id,
        "severity": one.severity,
        "value": value,
        "alert_threshold": rule.definition.alert_threshold,
        "warning_threshold": rule.definition.warning_threshold,
        "reminder": one.telling == Telling::Reminder,
        "trace": shown.map(|trace| trace_json(state, rule, trace)),
        "traces_url": traces_url,
    });
    let key = incident_key(one.incident_id, one.change);
    // An opening, or its reminder, is what a person may act on where it is told.
    let trace_url = shown.and_then(|trace| trace_link(state, rule, &trace.trace_id));
    let incident = (one.change == Move::Opened).then_some(IncidentNotice {
        id: one.incident_id,
        trace_url: trace_url.as_deref(),
    });
    let notice = |dedup_key| Notice {
        kind: kind_of(one.change),
        subject_user_id: None,
        title: &title,
        body: &body,
        metadata: metadata.clone(),
        dedup_key,
        incident,
    };
    let of = Of {
        organization_id: &rule.organization_id,
        project_id: &rule.project_id,
        owner: Owner::Rule(rule.id),
    };
    let reached = told(state, of, hearers, notice, &key, one.telling.again()).await;
    match reached {
        Ok(reached) => {
            let trace = shown
                .filter(|_| reached.anyone)
                .map(|trace| trace.trace_id.as_str());
            keep_told(
                state,
                (
                    &rule.organization_id,
                    &rule.project_id,
                    Owner::Rule(rule.id),
                ),
                one.incident_id,
                &reached.connections,
                trace,
                one.telling,
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

/// The kind of notice a move is told as.
const fn kind_of(change: Move) -> NotificationKind {
    match change {
        Move::Opened => NotificationKind::IncidentOpened,
        Move::Resolved => NotificationKind::IncidentResolved,
    }
}

/// The feed's key for an incident's opening or end: one row of each, so a
/// member's resolution and the loop's recovery are one.
fn incident_key(incident_id: Uuid, change: Move) -> String {
    format!("telemetry:incident:{incident_id}:{}", change.word())
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
        anyone: !routed.emitted.replayed(),
        connections: routed.connections,
    })
}

/// Where one of an incident's notices went, and the trace it showed, kept on
/// the incident, and on its alert or monitor as its last notice's hearers — a move's in
/// place of the last, a reminder's beside them; logged by its ids where it
/// cannot be, as the notice stands.
async fn keep_told(
    state: &AppState,
    (organization_id, project_id, owner): (&OrganizationId, &ProjectId, Owner),
    incident_id: Uuid,
    connections: &[Uuid],
    trace: Option<&str>,
    telling: Telling,
) {
    let replaces = telling != Telling::Reminder;
    if !replaces && connections.is_empty() && trace.is_none() {
        return;
    }
    let write = async {
        let mut tx = project_scope(&state.db, project_id)
            .await?
            .bind_organization(organization_id)
            .await?;
        incidents::told(&mut tx, project_id, incident_id, connections, trace).await?;
        let sql = match owner {
            Owner::Rule(_) => {
                "UPDATE telemetry.rules
                    SET told_to = CASE WHEN $3 THEN $2::uuid[]
                                  ELSE ARRAY(SELECT DISTINCT unnest(told_to || $2::uuid[]) ORDER BY 1) END
                  WHERE id = $1 AND project_id = $4"
            }
            Owner::Monitor(_) => {
                "UPDATE telemetry.monitors
                    SET told_to = CASE WHEN $3 THEN $2::uuid[]
                                  ELSE ARRAY(SELECT DISTINCT unnest(told_to || $2::uuid[]) ORDER BY 1) END
                  WHERE id = $1 AND project_id = $4"
            }
        };
        let (Owner::Rule(id) | Owner::Monitor(id)) = owner;
        sqlx::query(sql)
            .bind(id)
            .bind(connections)
            .bind(replaces)
            .bind(project_id)
            .execute(tx.conn())
            .await?;
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

/// Whether a maintenance window mutes the alert or monitor at `at`; none,
/// logged, where the windows cannot be read, so a notice is told rather than
/// lost.
async fn muted_at(
    state: &AppState,
    project_id: &ProjectId,
    owner: Owner,
    at: DateTime<Utc>,
) -> bool {
    let read = async {
        let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
        let muted = mutes::in_force(&mut tx, project_id, owner, at).await?;
        tx.commit().await?;
        Ok::<_, TelmoniError>(muted)
    };
    match read.await {
        Ok(muted) => muted,
        Err(error) => {
            tracing::warn!(
                project_id = %project_id,
                error = %error,
                "telemetry: a project's maintenance windows could not be read; it is told"
            );
            false
        }
    }
}

/// A notice's body, a line for each of what else it carries, then the changes
/// before it that fit [`BODY_MAX`]: last, so a connector cutting a long body
/// cuts them and never a link.
fn carrying<const N: usize>(
    body: String,
    lines: [Option<String>; N],
    changed: Option<&Changed>,
) -> String {
    let mut lines: Vec<String> = std::iter::once(body)
        .chain(lines.into_iter().flatten())
        .collect();
    if let Some(changed) = changed {
        let taken: usize = lines.iter().map(|line| chars(line) + 1).sum();
        lines.extend(changed_lines(changed, (BODY_MAX + 1).saturating_sub(taken)));
    }
    lines.join("\n")
}

/// The most characters a notice's body holds, as the seam's `Notice` has it.
const BODY_MAX: usize = 4_000;

/// The most changes a notice names; its incident names the rest.
const CHANGES_SHOWN: usize = 5;

/// How long a notice waits for its changes before it goes without them.
const CHANGES_BUDGET: Duration = Duration::from_secs(2);

fn chars(line: &str) -> usize {
    line.chars().count()
}

/// What an incident's agents changed to in the [`BEFORE_INCIDENT`] before it
/// opened, as its notice names them.
#[derive(Debug, Clone, PartialEq)]
struct Changed {
    opened_at: DateTime<Utc>,
    /// The newest first.
    changes: Vec<Change>,
    /// Whether its incident had more than it names.
    more: bool,
    /// Whether each names its agent, the incident being of no single one.
    several: bool,
}

/// The changes an evaluation's breach names, read once as its trace is; none
/// where no move shows them.
async fn changed_of(
    state: &AppState,
    project_id: &ProjectId,
    scope: &Scope,
    told: &[Told],
) -> Option<Changed> {
    let one = told
        .iter()
        .find(|one| one.telling != Telling::Silent && shows_trace(one))?;
    let incident_id = one.incident_id;
    let read = async {
        let mut tx = project_scope(&state.db, project_id).await?;
        let Some(incident) = incidents::one(&mut tx, project_id, incident_id).await? else {
            return Ok(None);
        };
        let asked = Opened {
            incident_id,
            opened_at: incident.opened_at,
            scope: scope.clone(),
        };
        // The incident's own read, so its notice counts what it names.
        let mut around = changes::around_incidents(&mut tx, project_id, &[asked]).await?;
        tx.commit().await?;
        let (changes, more) = around.remove(&incident_id).unwrap_or_default();
        Ok::<_, TelmoniError>(Some(Changed {
            opened_at: incident.opened_at,
            changes,
            more,
            several: scope.several(),
        }))
    };
    changed_within(CHANGES_BUDGET, incident_id, read)
        .await
        .filter(|changed| !changed.changes.is_empty())
}

/// What `read` answers inside `budget`; none, logged by the incident's id,
/// where it fails or runs past it, so a notice is never held for its changes.
async fn changed_within(
    budget: Duration,
    incident_id: Uuid,
    read: impl Future<Output = Result<Option<Changed>, TelmoniError>>,
) -> Option<Changed> {
    match tokio::time::timeout(budget, read).await {
        Ok(Ok(changed)) => changed,
        Ok(Err(error)) => {
            tracing::warn!(
                incident_id = %incident_id,
                error = %error,
                "telemetry: the changes before an incident could not be read; its notice goes without"
            );
            None
        }
        Err(_) => {
            tracing::warn!(
                incident_id = %incident_id,
                "telemetry: the changes before an incident were not read in time; its notice goes without"
            );
            None
        }
    }
}

/// The newest [`CHANGES_SHOWN`] changes as lines within `room` characters,
/// breaks counted, under a lead and over a count of the rest; none where none fits.
fn changed_lines(changed: &Changed, room: usize) -> Vec<String> {
    let named: Vec<String> = changed
        .changes
        .iter()
        .take(CHANGES_SHOWN)
        .map(|change| change_line(change, changed.opened_at, changed.several))
        .collect();
    for shown in (1..=named.len()).rev() {
        let rest = changed.changes.len().saturating_sub(shown);
        let more = match (rest, changed.more) {
            (0, false) => None,
            (rest, false) => Some(format!("and {rest} more on the incident")),
            (rest, true) => Some(format!("and over {rest} more on the incident")),
        };
        let lines: Vec<String> = std::iter::once(format!(
            "Changed in the {} hours before it opened:",
            BEFORE_INCIDENT.num_hours()
        ))
        .chain(named.iter().take(shown).cloned())
        .chain(more)
        .collect();
        if lines.iter().map(|line| chars(line) + 1).sum::<usize>() <= room {
            return lines;
        }
    }
    Vec::new()
}

/// One change as a notice names it: `release 2026.11.3 in production, 14
/// minutes before`, its agent first where the incident is of several.
fn change_line(change: &Change, opened_at: DateTime<Utc>, several: bool) -> String {
    let what = match change.kind {
        ChangeKind::Release => format!("release {}", clipped(&change.value)),
        ChangeKind::Model => format!("model {}", clipped(&change.value)),
        ChangeKind::Prompt => format!(
            "prompt {} version {}",
            clipped(change.prompt.as_deref().unwrap_or_default()),
            clipped(&change.value)
        ),
    };
    let who = if several {
        format!("{}: ", clipped(&change.agent))
    } else {
        String::new()
    };
    let whereabouts = if change.environment.is_empty() {
        String::new()
    } else {
        format!(" in {}", clipped(&change.environment))
    };
    // Its day marked no more of its kind, so it may not be the latest.
    let capped = if change.capped {
        ", and more that day"
    } else {
        ""
    };
    format!(
        "{who}{what}{whereabouts}, {}{capped}",
        before(opened_at - change.changed_at)
    )
}

/// How long before, in whole minutes under an hour and whole hours past it.
fn before(ago: TimeDelta) -> String {
    match ago.num_minutes() {
        ..=0 => "under a minute before".to_owned(),
        1 => "1 minute before".to_owned(),
        minutes @ 2..=59 => format!("{minutes} minutes before"),
        60..=119 => "1 hour before".to_owned(),
        minutes => format!("{} hours before", minutes / 60),
    }
}

/// Whether a move shows the trace, and the changes before its incident: a
/// breach opened, reopened or risen, which a window with no span cannot
/// show; a recovery need not, nor a reminder, which may come every minute
/// and whose opening named them.
fn shows_trace(one: &Told) -> bool {
    one.change == Move::Opened
        && one.severity != Severity::NoData
        && one.telling != Telling::Reminder
}

/// The trace an evaluation's breaches show, read once; none, logged by the
/// error's kind, where a store does not answer, and the notice goes on.
async fn trace_of(state: &AppState, rule: &RuleRow, told: &[Told]) -> Option<ExampleTrace> {
    if !told
        .iter()
        .any(|one| one.telling != Telling::Silent && shows_trace(one))
    {
        return None;
    }
    match example::example(state, rule, Utc::now()).await {
        Ok(trace) => trace,
        Err(Unread::Store(error)) => {
            tracing::warn!(
                rule_id = %rule.id,
                kind = crate::store::Fault::of(&error).as_str(),
                "telemetry: the trace an alert's notice shows could not be read; it goes without"
            );
            None
        }
        Err(Unread::Db(error)) => {
            tracing::warn!(
                rule_id = %rule.id,
                error = %error,
                "telemetry: the prices of the trace an alert's notice shows could not be read; \
                 it goes without"
            );
            None
        }
    }
}

/// The console's trace list as the alert read it, spelled with ids: its
/// filters, its window in days, and where it counts failures, those that
/// failed. The list has no filter by provider or source, so an alert's widens
/// it; none past [`LINK_MAX`], whose filters would run the notice past its
/// bounds.
fn traces_link(state: &AppState, rule: &RuleRow) -> Option<String> {
    let origin = state.config.app_url.as_deref()?;
    let mut url = reqwest::Url::parse(&format!(
        "{origin}/{}/{}/traces",
        rule.organization_id, rule.project_id
    ))
    .ok()?;
    let filters = &rule.definition.filters;
    let failed = vec![SpanStatus::Error];
    // An objective's traces miss it by failing, but a latency one's by running long.
    let failures = match rule.definition.objective {
        Some(objective) => objective.latency_ms.is_none(),
        None => rule.definition.measure == RuleMeasure::Failures,
    };
    let statuses = if filters.status.is_empty() && failures {
        &failed
    } else {
        &filters.status
    };
    {
        let mut query = url.query_pairs_mut();
        for fault in &filters.fault {
            query.append_pair("fault", &fault.to_string());
        }
        // The month's days so far, back to its first, which no fixed count covers.
        let days = match rule.definition.window {
            RuleWindow::Hour | RuleWindow::Day => 1,
            RuleWindow::Week => 7,
            RuleWindow::Month => chrono::Datelike::day(&Utc::now()),
        };
        query.append_pair("days", &days.to_string());
        for (facet, values) in [
            (Facet::Agent, &filters.agent),
            (Facet::Model, &filters.model),
            (Facet::Tool, &filters.tool),
            (Facet::Environment, &filters.environment),
            (Facet::Release, &filters.release),
        ] {
            for value in values {
                query.append_pair(facet.as_str(), value);
            }
        }
        for status in statuses {
            query.append_pair(Facet::Status.as_str(), &status.to_string());
        }
    }
    let url: String = url.into();
    (url.len() <= LINK_MAX).then_some(url)
}

/// The longest trace list link a notice carries, in bytes.
const LINK_MAX: usize = 1_024;

/// A trace's own page in the console, spelled with ids.
fn trace_link(state: &AppState, rule: &RuleRow, trace_id: &str) -> Option<String> {
    trace_page(state, &rule.organization_id, &rule.project_id, trace_id)
}

/// A trace's own page in the console, by the project's ids.
fn trace_page(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    trace_id: &str,
) -> Option<String> {
    let origin = state.config.app_url.as_deref()?;
    Some(format!(
        "{origin}/{organization_id}/{project_id}/traces/{trace_id}"
    ))
}

/// The line naming the trace a notice shows: which it is, or the loop it
/// shows at the threshold `severity` crossed, its last step, its error, its
/// steps, its tokens and what it spent, the models it called that no price
/// covers, and where it opens.
fn trace_line(
    state: &AppState,
    rule: &RuleRow,
    trace: &ExampleTrace,
    severity: Severity,
) -> String {
    let label = trace.repeated.as_ref().map_or_else(
        || picked(rule.definition.kind, trace.pick),
        |repeated| repeated_words(&rule.definition, repeated, severity),
    );
    let mut parts = Vec::new();
    if let Some(step) = &trace.last_step {
        parts.push(format!("last step {}", clipped(step)));
    }
    if let Some(error) = &trace.error_type {
        parts.push(clipped(error));
    }
    parts.push(format!(
        "{} {}",
        trace.steps,
        if trace.steps == 1 { "step" } else { "steps" }
    ));
    parts.push(format!(
        "{} tokens and {}{}",
        trace.tokens,
        dollars(trace.spent),
        if trace.ended { "" } else { " so far" }
    ));
    if !trace.unpriced.is_empty() {
        parts.push(format!("no price for {}", unpriced_names(&trace.unpriced)));
    }
    let details = parts.join(", ");
    match trace_link(state, rule, &trace.trace_id) {
        Some(url) => format!("{label}: {details}. {url}"),
        None => format!("{label} ({}): {details}.", trace.trace_id),
    }
}

/// A loop in words, its traces counted at the threshold `severity` crossed:
/// "3 traces of support-agent called search 10 or more times in a row, this one 12".
fn repeated_words(
    definition: &crate::rules::Definition,
    repeated: &Repeated,
    severity: Severity,
) -> String {
    let (at, traces) = match (severity, definition.warning_threshold) {
        (Severity::Warning, Some(warning)) => (warning, repeated.past_warning),
        _ => (definition.alert_threshold, repeated.past_alert),
    };
    let called = format!(
        "{} called {}",
        clipped(&repeated.agent),
        clipped(&repeated.tool)
    );
    let bound = match definition.operator {
        RuleOperator::Gt => format!("more than {}", amount(definition, at)),
        RuleOperator::Gte => format!("{} or more", amount(definition, at)),
        _ => String::new(),
    };
    if bound.is_empty() || traces == 0 {
        return format!("The trace where {called} {} times in a row", repeated.times);
    }
    let traces = if traces == 1 {
        "1 trace".to_owned()
    } else {
        format!("{traces} traces")
    };
    format!(
        "{traces} of {called} {bound} times in a row, this one {}",
        repeated.times
    )
}

/// The most unpriced names a trace's line carries, the rest counted.
const UNPRICED_SHOWN: usize = 3;

/// A trace's unpriced names in words: `gpt-x (openai), ft-model (openai) and 2 more`.
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

/// The trace a notice shows, as its metadata carries it: names clipped and
/// the unpriced models counted, so it stays within what the feed keeps.
fn trace_json(state: &AppState, rule: &RuleRow, trace: &ExampleTrace) -> serde_json::Value {
    json!({
        "trace_id": trace.trace_id,
        "url": trace_link(state, rule, &trace.trace_id),
        "last_step": trace.last_step.as_deref().map(clipped),
        "error_type": trace.error_type.as_deref().map(clipped),
        "steps": trace.steps,
        "tokens": trace.tokens,
        "spent": trace.spent,
        "unpriced": trace.unpriced.len(),
        "ended": trace.ended,
    })
}

/// Which trace a notice shows, in words.
fn picked(kind: SpanKind, pick: Pick) -> String {
    let one = match kind {
        SpanKind::Agent => "trace",
        SpanKind::TraceStart => "trace opening",
        SpanKind::Step => "step",
        SpanKind::ModelCall => "model call",
        SpanKind::ToolCall => "tool call",
        SpanKind::Retrieval => "retrieval",
        SpanKind::Span => "span",
    };
    match (pick, kind) {
        (Pick::LatestFailed, SpanKind::Agent) => "The latest failed trace".to_owned(),
        (Pick::LatestFailed, _) => format!("The trace of the latest failed {one}"),
        (Pick::Slowest, SpanKind::Agent) => "The slowest trace".to_owned(),
        (Pick::Slowest, _) => format!("The trace of the slowest {one}"),
        (Pick::MostTokens, SpanKind::Agent) => "The trace with the most tokens".to_owned(),
        (Pick::MostTokens, _) => format!("The trace of the {one} with the most tokens"),
        (Pick::MostSpent, _) => "The trace that spent the most".to_owned(),
        (Pick::LowestScored, _) => "The trace scored lowest".to_owned(),
        (Pick::HighestScored, _) => "The trace scored highest".to_owned(),
        (Pick::MostSteps, _) => "The trace with the most steps".to_owned(),
        (Pick::LongestRepeat, _) => {
            "The trace that called one tool the most times in a row".to_owned()
        }
        (Pick::Stalest, _) => "The trace quiet longest".to_owned(),
        (Pick::Latest, SpanKind::Agent) => "The latest trace".to_owned(),
        (Pick::Latest, _) => format!("The trace of the latest {one}"),
        (Pick::LatestFault, _) => format!("The trace of the latest {one} with a selected fault"),
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

/// The connectors the alert or monitor links, read in the module's lane.
async fn links_of(state: &AppState, owner: Owner) -> Result<Vec<Link>, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let links = match owner {
        Owner::Rule(id) => db::links_of(&mut tx, id)
            .await?
            .into_iter()
            .map(db::LinkRow::link)
            .collect(),
        Owner::Monitor(id) => crate::monitors::links_of(&mut tx, id).await?,
    };
    tx.commit().await?;
    Ok(links)
}

/// Whose notice: the organization and project it is filed under, and the
/// alert or monitor whose route it goes by.
#[derive(Clone, Copy)]
struct Of<'a> {
    organization_id: &'a OrganizationId,
    project_id: &'a ProjectId,
    owner: Owner,
}

/// `notice` told to `hearers` within the day's room, under the alert's
/// route, the room settled after by what was made; keyed in the feed as
/// `key`, or to the route alone where it is said `again`.
async fn told<'a>(
    state: &AppState,
    of: Of<'_>,
    hearers: &[Uuid],
    notice: impl Fn(Option<&'a str>) -> Notice<'a>,
    key: &'a str,
    again: bool,
) -> Result<Reached, TelmoniError> {
    let taken = deliveries::take(state, of.organization_id, of.project_id, hearers.len()).await;
    let route_key = of.owner.route_key();
    let route = Route {
        key: &route_key,
        connections: hearers,
        most: taken.room(),
    };
    let reached = tell(
        state,
        of.organization_id,
        of.project_id,
        notice,
        key,
        route,
        again,
    )
    .await;
    let made = reached
        .as_ref()
        .map_or(0, |reached| reached.connections.len());
    deliveries::settle(state, of.organization_id, taken, made).await;
    reached
}

/// A notice's title and body: the alert's name and where it stands, and its
/// metric in words with the value that set it — numbers alone, never a
/// span's content.
fn words(rule: &RuleRow, value: Option<f64>, told: &Told) -> (String, String) {
    if let Some(objective) = rule.definition.objective {
        return burn_words(rule, &objective, value, told);
    }
    let what = metric(rule);
    let shown = value.map_or_else(|| "no value".to_owned(), |v| amount(&rule.definition, v));
    let threshold = |t: f64| amount(&rule.definition, t);
    // An incident held through a window with no span has no value to give.
    let past = |level: &str, at: String| match value {
        Some(_) => format!("{what} is {shown}, past the {level} threshold of {at}."),
        None => format!(
            "{what} has no value now; its incident stands past the {level} threshold of {at}."
        ),
    };
    let (state, body) = match (told.change, told.severity) {
        (Move::Resolved, _) => (
            "resolved",
            format!("{what} is {shown}, inside its thresholds again."),
        ),
        (Move::Opened, Severity::NoData) => (
            "no data",
            match (rule.definition.baseline, rule.definition.measure) {
                (RuleBaseline::Previous, _) => format!(
                    "{what} has no change to read: it or the window before held nothing to compare."
                ),
                (_, RuleMeasure::Score) => {
                    format!("{what} has had no score to read for as long as its window.")
                }
                _ => format!("{what} has had no span to read for as long as its window."),
            },
        ),
        (Move::Opened, Severity::Warning) => (
            "warning",
            match rule.definition.warning_threshold {
                Some(at) => past("warning", threshold(at)),
                // An edit took the warning threshold away; the incident stands
                // until the next reading moves it.
                None => match value {
                    Some(_) => {
                        format!("{what} is {shown}, at warning, whose threshold was removed.")
                    }
                    None => format!(
                        "{what} has no value now; its incident stands at warning, whose threshold was removed."
                    ),
                },
            },
        ),
        (Move::Opened, _) => (
            "alert",
            past("alert", threshold(rule.definition.alert_threshold)),
        ),
    };
    let body = match (told.change, told.severity, value) {
        (Move::Opened, Severity::Warning, Some(v)) => {
            format!(
                "{body}{}",
                forecast(&rule.definition, v, Utc::now()).unwrap_or_default()
            )
        }
        _ => body,
    };
    let lead = if told.telling == Telling::Reminder {
        "Still: "
    } else {
        ""
    };
    (
        format!("{}: {state}", rule.definition.name),
        format!("{lead}{body}"),
    )
}

/// An objective's notice: how fast its agent's traces spend its budget, over
/// the window that set the severity, and when the budget is gone at that rate.
fn burn_words(
    rule: &RuleRow,
    objective: &crate::rules::Objective,
    value: Option<f64>,
    told: &Told,
) -> (String, String) {
    let definition = &rule.definition;
    let agent = definition.filters.agent.first().map_or("", String::as_str);
    let promise = promise(objective, agent);
    let burn = value.unwrap_or(0.0);
    let shown = amount(definition, burn);
    let gone = |burn: f64| match burn > 0.0 {
        true => format!(
            " At this rate its {} days' budget is gone in {}.",
            objective.days,
            days(f64::from(objective.days) / burn)
        ),
        false => String::new(),
    };
    let (state, body) = match (told.change, told.severity) {
        (Move::Resolved, _) => (
            "resolved",
            format!(
                "{promise}: its budget burns at {shown} again, under {} over the last hour and {}                  over the last six hours.",
                amount(definition, definition.alert_threshold),
                amount(definition, definition.warning_threshold.unwrap_or_default()),
            ),
        ),
        (_, Severity::Warning) => (
            "warning",
            format!(
                "{promise}: its budget burns at {shown} over the last six hours, past the {} that                  warns.{}",
                amount(definition, definition.warning_threshold.unwrap_or_default()),
                gone(burn)
            ),
        ),
        _ => (
            "alert",
            format!(
                "{promise}: its budget burns at {shown} over the last hour, past the {} that                  pages.{}",
                amount(definition, definition.alert_threshold),
                gone(burn)
            ),
        ),
    };
    let lead = if told.telling == Telling::Reminder {
        "Still: "
    } else {
        ""
    };
    (
        format!("{}: {state}", definition.name),
        format!("{lead}{body}"),
    )
}

/// What an objective promises, in words: "99.9% of support-bot's traces
/// succeed over 30 days".
fn promise(objective: &crate::rules::Objective, agent: &str) -> String {
    let share = format!("{:.2}", objective.target * 100.0);
    let share = share.trim_end_matches('0').trim_end_matches('.');
    let how = match objective.latency_ms {
        Some(ms) => format!("end inside {ms} ms"),
        None => "succeed".to_owned(),
    };
    format!(
        "{share}% of {agent}'s traces {how} over {} days",
        objective.days
    )
}

/// A span of days in words, to a tenth under ten, else whole; under a day in hours.
fn days(days: f64) -> String {
    if days < 1.0 {
        return format!("{:.0} hours", (days * 24.0).max(1.0));
    }
    if days < 10.0 {
        return format!("{days:.1} days");
    }
    format!("{days:.0} days")
}

/// The alert's metric in words: "The p95 duration of model calls over the
/// last hour", "The most a trace spent over the last day", and against the
/// window before, "…, against the day before".
fn metric(rule: &RuleRow) -> String {
    let what = measured(rule);
    match (rule.definition.baseline, rule.definition.window) {
        (RuleBaseline::Previous, window) => format!("{what}, against the {window} before"),
        (RuleBaseline::None, _) => what,
    }
}

/// The alert's measure folded over its window, in words.
fn measured(rule: &RuleRow) -> String {
    if rule.definition.measure == RuleMeasure::Score {
        let name = rule.definition.score_name.as_deref().unwrap_or_default();
        let folded = match rule.definition.aggregation {
            RuleAggregation::Count => format!("The count of {name} scores"),
            RuleAggregation::Avg => format!("The average {name} score"),
            RuleAggregation::Max | RuleAggregation::Sum => format!("The highest {name} score"),
            percentile => format!("The {percentile} {name} score"),
        };
        return format!("{folded} {}", over(rule.definition.window));
    }
    let folded = match (rule.definition.measure, rule.definition.aggregation) {
        (RuleMeasure::Count, _) => "The count".to_owned(),
        (RuleMeasure::Failures, RuleAggregation::Avg) => "The share failed".to_owned(),
        (RuleMeasure::Failures, _) => "The failures".to_owned(),
        (RuleMeasure::Faults, RuleAggregation::Avg) => "The share with a selected fault".to_owned(),
        (RuleMeasure::Faults, _) => "The selected faults".to_owned(),
        (RuleMeasure::Duration, RuleAggregation::Avg) => "The average duration".to_owned(),
        (RuleMeasure::Duration, RuleAggregation::Sum) => "The summed duration".to_owned(),
        (RuleMeasure::Duration, percentile) => format!("The {percentile} duration"),
        (RuleMeasure::Tokens, RuleAggregation::Avg) => "The average tokens".to_owned(),
        (RuleMeasure::Tokens, _) => "The tokens".to_owned(),
        (RuleMeasure::Cost, aggregation) => {
            let spent = match aggregation {
                RuleAggregation::Max => "The most a trace spent",
                RuleAggregation::Avg => "The average a trace spent",
                _ => "What the traces spent",
            };
            return format!("{spent} {}", over(rule.definition.window));
        }
        (RuleMeasure::Steps, aggregation) => {
            let steps = match aggregation {
                RuleAggregation::Avg => "The average steps a trace took".to_owned(),
                RuleAggregation::Max => "The most steps a trace took".to_owned(),
                percentile => format!("The {percentile} of the steps a trace took"),
            };
            return format!("{steps} {}", over(rule.definition.window));
        }
        (RuleMeasure::Repeats, _) => {
            return format!(
                "The most times a trace called one tool in a row {}",
                over(rule.definition.window)
            );
        }
        (RuleMeasure::Stalled, _) => {
            let quiet = match rule.definition.window {
                RuleWindow::Hour => "an hour",
                RuleWindow::Day => "a day",
                RuleWindow::Week => "a week",
                RuleWindow::Month => "a month",
            };
            return format!("The count of traces left open and quiet for over {quiet}");
        }
        // Worded above, by its score's name, and an objective by its promise.
        (RuleMeasure::Score | RuleMeasure::Burn, _) => String::new(),
    };
    let of = match rule.definition.kind {
        SpanKind::Agent => "traces",
        SpanKind::TraceStart => "trace openings",
        SpanKind::Step => "steps",
        SpanKind::ModelCall => "model calls",
        SpanKind::ToolCall => "tool calls",
        SpanKind::Retrieval => "retrievals",
        SpanKind::Span => "spans",
    };
    format!("{folded} of {of} {}", over(rule.definition.window))
}

/// How far back an alert's window reaches, in words.
fn over(window: RuleWindow) -> String {
    match window {
        RuleWindow::Month => "so far this month".to_owned(),
        trailing => format!("over the last {trailing}"),
    }
}

/// The day this month a budget's burn so far reaches its alert threshold, or that it
/// stays under; none for an alert that is no budget.
fn forecast(
    definition: &crate::rules::Definition,
    value: f64,
    now: DateTime<Utc>,
) -> Option<String> {
    if !(definition.measure == RuleMeasure::Cost
        && definition.aggregation == RuleAggregation::Sum
        && definition.window == RuleWindow::Month)
    {
        return None;
    }
    let threshold = definition.alert_threshold;
    if value <= 0.0 || value >= threshold {
        return None;
    }
    let window = crate::rules::evaluate::Window::of(now, RuleWindow::Month);
    let elapsed = (now - window.from).num_seconds();
    if elapsed <= 0 {
        return None;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a month's seconds are exact in f64"
    )]
    let reached_in = (threshold / value) * elapsed as f64;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a forecast is to the second"
    )]
    let reached = window.from + chrono::Duration::seconds(reached_in as i64);
    let next_month = window
        .from
        .checked_add_months(chrono::Months::new(1))
        .unwrap_or(window.from);
    let at = amount(definition, threshold);
    Some(if reached < next_month {
        format!(
            " At this rate it reaches {at} on {}.",
            reached.format("%B %-d")
        )
    } else {
        format!(" At this rate it stays under {at} this month.")
    })
}

/// A value as a notice writes it: a change from the window before as a
/// percentage, durations in milliseconds, a share as a percentage, a cost in
/// dollars, the rest as numbers, to two places at most.
fn amount(definition: &crate::rules::Definition, value: f64) -> String {
    let number = |v: f64| {
        let text = format!("{v:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    };
    if definition.baseline == RuleBaseline::Previous {
        return format!("{}%", number(value));
    }
    match (definition.measure, definition.aggregation) {
        (RuleMeasure::Duration, _) => format!("{} ms", number(value)),
        (RuleMeasure::Failures | RuleMeasure::Faults, RuleAggregation::Avg) => {
            format!("{}%", number(value * 100.0))
        }
        (RuleMeasure::Cost, _) => dollars(value),
        (RuleMeasure::Burn, _) => format!("{}×", number(value)),
        _ => number(value),
    }
}

/// US dollars as a notice writes them: to the cent from a dollar, else to
/// three figures, since a trace's is often a fraction of a cent (`$5.20`,
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

/// A monitor's moves, told after their commit as an alert's are: an opening
/// and a resolution to the feed and its linked connectors, a rise to them
/// alone; a failure shows the trace that failed, and an opening what the
/// agent changed to before it. Names the agent, the due time, the trace's id
/// and those changes alone.
pub async fn tell_monitor(
    state: &AppState,
    row: &MonitorRow,
    judged: &Judged,
    moves: &[Told],
    at: DateTime<Utc>,
) {
    if moves.iter().all(|one| one.telling == Telling::Silent) {
        return;
    }
    let owner = Owner::Monitor(row.id);
    let links = match links_of(state, owner).await {
        Ok(links) => links,
        Err(error) => {
            tracing::warn!(
                monitor_id = %row.id,
                error = %error,
                "telemetry: where a monitor's notices go could not be read; the feed alone hears them"
            );
            Vec::new()
        }
    };
    let muted = muted_at(state, &row.project_id, owner, at).await;
    let scope = Scope::of_monitor(&row.agent);
    let changed = changed_of(state, &row.project_id, &scope, moves).await;
    for one in moves.iter().filter(|one| one.telling != Telling::Silent) {
        let hearers: Vec<Uuid> = links
            .iter()
            .filter(|link| link.hears_end(one.heard_at, &one.told_to, muted, at))
            .map(|link| link.connection_id)
            .collect();
        let failed = one.change == Move::Opened && judged.cause == Some(MonitorCause::Failed);
        let trace = judged
            .last_ended
            .as_ref()
            .filter(|_| failed)
            .map(|ended| ended.trace_id.as_str());
        let trace_url =
            trace.and_then(|trace| trace_page(state, &row.organization_id, &row.project_id, trace));
        let (title, body) = monitor_words(row, judged, one);
        let body = carrying(
            body,
            [trace_url.as_ref().map(|url| format!("Trace: {url}"))],
            changed.as_ref().filter(|_| shows_trace(one)),
        );
        let metadata = json!({
            "incident_id": one.incident_id,
            "monitor_id": row.id,
            "severity": one.severity,
            "cause": (one.change == Move::Opened).then_some(judged.cause).flatten(),
            "next_due_at": judged.next_due_at,
            "trace": trace,
        });
        let key = incident_key(one.incident_id, one.change);
        let incident = (one.change == Move::Opened).then_some(IncidentNotice {
            id: one.incident_id,
            trace_url: trace_url.as_deref(),
        });
        let notice = |dedup_key| Notice {
            kind: kind_of(one.change),
            subject_user_id: None,
            title: &title,
            body: &body,
            metadata: metadata.clone(),
            dedup_key,
            incident,
        };
        let of = Of {
            organization_id: &row.organization_id,
            project_id: &row.project_id,
            owner,
        };
        match told(state, of, &hearers, notice, &key, one.telling.again()).await {
            Ok(reached) => {
                let shown = trace.filter(|_| reached.anyone);
                keep_told(
                    state,
                    (&row.organization_id, &row.project_id, owner),
                    one.incident_id,
                    &reached.connections,
                    shown,
                    one.telling,
                )
                .await;
            }
            Err(error) => tracing::warn!(
                monitor_id = %row.id,
                incident_id = %one.incident_id,
                error = %error,
                "telemetry: a monitor's notice could not be told"
            ),
        }
    }
}

/// A monitor's notice's title and body: its agent, where it stands and why,
/// and when its trace was due — never a span's content.
fn monitor_words(row: &MonitorRow, judged: &Judged, one: &Told) -> (String, String) {
    let agent = &row.agent;
    let due = judged.next_due_at.map_or_else(String::new, |due| {
        due.format("%Y-%m-%d %H:%M UTC").to_string()
    });
    let grace = minutes(row.grace_seconds);
    let (state, body) = match (one.change, one.severity, judged.cause) {
        (Move::Resolved, ..) => ("up", format!("{agent} is up again.")),
        (Move::Opened, _, Some(MonitorCause::Failed)) => {
            ("down", format!("{agent}'s last trace ended in error."))
        }
        (Move::Opened, Severity::Warning, _) => (
            "late",
            format!("{agent}'s trace due at {due} has not started."),
        ),
        (Move::Opened, ..) => (
            "down",
            format!("{agent}'s trace due at {due} has not started within its grace of {grace}."),
        ),
    };
    (format!("{agent}: {state}"), body)
}

/// A grace in words: whole minutes, hours or days where they divide it.
fn minutes(seconds: i32) -> String {
    let minutes = seconds / 60;
    match minutes {
        m if m % 1_440 == 0 => format!("{} days", m / 1_440).replace("1 days", "1 day"),
        m if m % 60 == 0 => format!("{} hours", m / 60).replace("1 hours", "1 hour"),
        1 => "1 minute".to_owned(),
        m => format!("{m} minutes"),
    }
}

/// A member's resolution of an incident, told as a recovery is: in the feed,
/// keyed as the incident's resolution, and to the linked connectors that hear
/// it or heard the incident; to them alone once it reopened.
pub async fn resolved_by_hand(state: &AppState, incident: &IncidentRow) {
    let Some(owner) = incident.owner() else {
        return;
    };
    let links = match links_of(state, owner).await {
        Ok(links) => links,
        Err(error) => {
            tracing::warn!(
                incident_id = %incident.id,
                error = %error,
                "telemetry: where a resolution goes could not be read; the feed alone hears it"
            );
            Vec::new()
        }
    };
    tell_resolved(state, incident, &links, false).await;
}

/// An open incident gone with the alert or monitor a member deleted, told as
/// a member's resolution is, to the connectors it linked, `links`.
pub async fn deleted_with(state: &AppState, incident: &IncidentRow, links: Vec<Link>) {
    tell_resolved(state, incident, &links, true).await;
}

/// A resolution by a member told to the feed and to those of `links` that
/// hear it, its alert or monitor `deleted` with it or not.
async fn tell_resolved(state: &AppState, incident: &IncidentRow, links: &[Link], deleted: bool) {
    let Some(owner) = incident.owner() else {
        return;
    };
    let (organization_id, project_id) = (&incident.organization_id, &incident.project_id);
    let (incident_id, severity) = (incident.id, incident.severity);
    let title = format!("{}: resolved", incident.named());
    let key = incident_key(incident.id, Move::Resolved);
    let mut metadata = json!({
        "incident_id": incident_id,
        "rule_id": incident.rule_id,
        "monitor_id": incident.monitor_id,
        "severity": severity,
        "by_hand": true,
    });
    let body = match (deleted, owner) {
        (true, Owner::Rule(_)) => {
            if let Some(fields) = metadata.as_object_mut() {
                fields.insert("alert_deleted".into(), json!(true));
            }
            "A member of the project deleted this alert, and its incident with it."
        }
        (true, Owner::Monitor(_)) => {
            if let Some(fields) = metadata.as_object_mut() {
                fields.insert("monitor_deleted".into(), json!(true));
            }
            "A member of the project deleted this monitor, and its incident with it."
        }
        (false, _) => "A member of the project resolved this incident.",
    };
    let notice = |dedup_key| Notice {
        kind: NotificationKind::IncidentResolved,
        subject_user_id: None,
        title: &title,
        body,
        metadata: metadata.clone(),
        dedup_key,
        incident: None,
    };
    let now = Utc::now();
    let muted = muted_at(state, project_id, owner, now).await;
    let hearers: Vec<Uuid> = links
        .iter()
        .filter(|link| link.hears_end(severity, &incident.told_to, muted, now))
        .map(|link| link.connection_id)
        .collect();
    let of = Of {
        organization_id,
        project_id,
        owner,
    };
    let reached = told(state, of, &hearers, notice, &key, incident.reopened > 0).await;
    match reached {
        // A deleted incident keeps nothing.
        Ok(_) if deleted => {}
        Ok(reached) => {
            keep_told(
                state,
                (organization_id, project_id, owner),
                incident_id,
                &reached.connections,
                None,
                Telling::First,
            )
            .await;
        }
        Err(error) => tracing::warn!(
            project_id = %project_id,
            incident_id = %incident_id,
            error = %error,
            "telemetry: an incident resolved by hand could not be told"
        ),
    }
}

/// A project's first trace told to its feed: the starter alerts it made, and
/// those the plan had no room for in the plan's own words; one that fails is
/// logged, the starters standing all the same.
pub(crate) async fn starters_made(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    made: &crate::starters::Made,
) {
    if !crate::handler::chain::open(state.auth.as_ref(), organization_id).await {
        return;
    }
    let title = if made.names.is_empty() {
        "No starter alerts made: the plan is full"
    } else {
        "Starter alerts watch this project"
    };
    let mut body = if made.names.is_empty() {
        "Its first trace landed, but the plan had no room for its starter alerts.".to_owned()
    } else {
        format!(
            "Its first trace landed, so {} now watch its traces, read every five minutes. Edit \
             or delete them on its Alerts page.",
            crate::starters::words(&made.names)
        )
    };
    if let Some(detail) = &made.detail {
        let names: Vec<String> = made
            .withheld
            .iter()
            .map(|measure| crate::starters::name_of(*measure))
            .collect();
        body = format!(
            "{body} Not made: {}. {detail}",
            crate::starters::words(&names)
        );
    }
    let dedup_key = format!("telemetry:starters:{project_id}");
    let withheld: Vec<String> = made.withheld.iter().map(ToString::to_string).collect();
    let notice = Notice {
        kind: NotificationKind::StarterAlerts,
        subject_user_id: None,
        title,
        body: &body,
        metadata: json!({ "made": made.names, "withheld": withheld }),
        dedup_key: Some(&dedup_key),
        incident: None,
    };
    if let Err(error) = state
        .notifications
        .emit(organization_id, project_id, notice)
        .await
    {
        tracing::warn!(
            project_id = %project_id,
            error = %error,
            "telemetry: a project's feed could not be told of its starter alerts"
        );
    }
}

#[cfg(test)]
mod tests {
    use telmoni_shared::{MissingData, RuleOperator};

    use crate::rules::{RuleFilters, RuleInput};

    /// A budget: what the traces spent this month, warned at 80 and alerting at 100.
    fn budget() -> crate::rules::Definition {
        RuleInput {
            name: "Budget".into(),
            kind: SpanKind::Agent,
            measure: RuleMeasure::Cost,
            aggregation: RuleAggregation::Sum,
            filters: RuleFilters::default(),
            window: RuleWindow::Month,
            operator: RuleOperator::Gt,
            alert_threshold: 100.0,
            warning_threshold: Some(80.0),
            missing_data: MissingData::Zero,
            renotify_minutes: None,
            score_name: None,
            baseline: telmoni_shared::RuleBaseline::None,
            objective: None,
            connectors: Vec::new(),
        }
        .checked()
        .expect("a budget")
        .definition
    }

    /// An objective's notice says what it promises, its burn as a multiple,
    /// and how long its budget lasts at that rate.
    #[test]
    fn an_objective_reads_as_its_promise_and_its_burn() {
        let success = crate::rules::Objective {
            target: 0.999,
            latency_ms: None,
            days: 30,
        };
        assert_eq!(
            promise(&success, "support-bot"),
            "99.9% of support-bot's traces succeed over 30 days"
        );
        let latency = crate::rules::Objective {
            target: 0.95,
            latency_ms: Some(2_000),
            ..success
        };
        assert_eq!(
            promise(&latency, "support-bot"),
            "95% of support-bot's traces end inside 2000 ms over 30 days"
        );
        let burn = crate::rules::Definition {
            measure: RuleMeasure::Burn,
            aggregation: RuleAggregation::Avg,
            ..budget()
        };
        assert_eq!(amount(&burn, 14.4), "14.4×");
        assert_eq!(days(30.0 / 15.0), "2.0 days");
        assert_eq!(days(30.0 / 300.0), "2 hours");
        assert_eq!(days(30.0 / 2.0), "15 days");
    }

    /// A budget's warning names the day its burn reaches the alert; no other alert forecasts.
    #[test]
    fn a_budget_forecasts_the_day_its_burn_reaches_the_alert() {
        let at = |day: u32| {
            chrono::NaiveDate::from_ymd_opt(2026, 11, day)
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .expect("a day")
                .and_utc()
        };
        // $80 in ten days reaches $100 on the 13th, at 12:00.
        assert_eq!(
            forecast(&budget(), 80.0, at(11)).as_deref(),
            Some(" At this rate it reaches $100.00 on November 13.")
        );
        // $10 in ten days stays under $100 in November.
        assert_eq!(
            forecast(&budget(), 10.0, at(11)).as_deref(),
            Some(" At this rate it stays under $100.00 this month.")
        );
        assert_eq!(forecast(&budget(), 120.0, at(11)), None, "already past");
        let trailing = crate::rules::Definition {
            window: RuleWindow::Week,
            ..budget()
        };
        assert_eq!(forecast(&trailing, 80.0, at(11)), None);
    }

    use super::*;

    #[test]
    fn amounts_read_as_their_measure() {
        let of = |measure, aggregation, value| {
            amount(
                &crate::rules::Definition {
                    measure,
                    aggregation,
                    ..budget()
                },
                value,
            )
        };
        assert_eq!(
            of(RuleMeasure::Duration, RuleAggregation::P95, 6_200.0),
            "6200 ms"
        );
        assert_eq!(of(RuleMeasure::Failures, RuleAggregation::Avg, 0.25), "25%");
        assert_eq!(
            of(RuleMeasure::Tokens, RuleAggregation::Avg, 101.333),
            "101.33"
        );
        assert_eq!(of(RuleMeasure::Cost, RuleAggregation::Max, 5.0), "$5.00");
        assert_eq!(of(RuleMeasure::Score, RuleAggregation::Avg, 0.8125), "0.81");
        let changed = crate::rules::Definition {
            measure: RuleMeasure::Duration,
            baseline: RuleBaseline::Previous,
            window: RuleWindow::Day,
            ..budget()
        };
        assert_eq!(
            amount(&changed, -12.5),
            "-12.5%",
            "a change reads in percent, whatever its measure"
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

    /// A trace's unpriced names are named, a few of them, and the rest counted.
    #[test]
    fn a_traces_unpriced_names_are_named_and_the_rest_counted() {
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

    /// A trace is named for the span it was picked by, in the alert's kind.
    #[test]
    fn a_trace_is_named_for_its_pick() {
        assert_eq!(
            picked(SpanKind::Agent, Pick::LatestFailed),
            "The latest failed trace"
        );
        assert_eq!(
            picked(SpanKind::ToolCall, Pick::LatestFailed),
            "The trace of the latest failed tool call"
        );
        assert_eq!(
            picked(SpanKind::Retrieval, Pick::LatestFailed),
            "The trace of the latest failed retrieval"
        );
        assert_eq!(
            picked(SpanKind::ModelCall, Pick::MostTokens),
            "The trace of the model call with the most tokens"
        );
        assert_eq!(
            picked(SpanKind::Agent, Pick::MostSpent),
            "The trace that spent the most"
        );
        assert_eq!(
            picked(SpanKind::Agent, Pick::MostSteps),
            "The trace with the most steps"
        );
        assert_eq!(
            picked(SpanKind::Agent, Pick::Stalest),
            "The trace quiet longest"
        );
    }

    /// A loop's traces are counted at the threshold crossed, as the alert compares.
    #[test]
    fn a_loop_counts_its_traces_at_the_threshold_crossed() {
        let repeats = crate::rules::Definition {
            measure: RuleMeasure::Repeats,
            aggregation: RuleAggregation::Max,
            window: RuleWindow::Hour,
            operator: RuleOperator::Gte,
            alert_threshold: 10.0,
            warning_threshold: Some(5.0),
            ..budget()
        };
        let looped = Repeated {
            agent: "support-agent".into(),
            tool: "search".into(),
            times: 12,
            past_alert: 3,
            past_warning: 4,
        };
        assert_eq!(
            repeated_words(&repeats, &looped, Severity::Alert),
            "3 traces of support-agent called search 10 or more times in a row, this one 12"
        );
        assert_eq!(
            repeated_words(&repeats, &looped, Severity::Warning),
            "4 traces of support-agent called search 5 or more times in a row, this one 12"
        );
        let above = crate::rules::Definition {
            operator: RuleOperator::Gt,
            ..repeats.clone()
        };
        let once = Repeated {
            past_alert: 1,
            ..looped.clone()
        };
        assert_eq!(
            repeated_words(&above, &once, Severity::Alert),
            "1 trace of support-agent called search more than 10 times in a row, this one 12"
        );
        let below = crate::rules::Definition {
            operator: RuleOperator::Lt,
            warning_threshold: None,
            ..repeats
        };
        assert_eq!(
            repeated_words(&below, &looped, Severity::Alert),
            "The trace where support-agent called search 12 times in a row"
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

    fn opened() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap_or_default()
    }

    /// A release of `agent` in production, first carried `ago` before [`opened`].
    fn released(agent: &str, value: &str, ago: TimeDelta) -> Change {
        Change {
            agent: agent.into(),
            environment: "production".into(),
            kind: ChangeKind::Release,
            prompt: None,
            value: value.into(),
            changed_at: opened() - ago,
            trace_id: "0".repeat(32),
            capped: false,
        }
    }

    fn changed(changes: Vec<Change>) -> Changed {
        Changed {
            opened_at: opened(),
            changes,
            more: false,
            several: false,
        }
    }

    /// A change says how long before the incident it was, in whole minutes then hours.
    #[test]
    fn a_change_says_how_long_before_the_incident_it_was() {
        let line = |ago| change_line(&released("support", "2026.11.3", ago), opened(), false);
        for (ago, words) in [
            (TimeDelta::zero(), "under a minute before"),
            (TimeDelta::seconds(59), "under a minute before"),
            (TimeDelta::seconds(60), "1 minute before"),
            (TimeDelta::minutes(14), "14 minutes before"),
            (TimeDelta::minutes(59), "59 minutes before"),
            (TimeDelta::minutes(60), "1 hour before"),
            (TimeDelta::minutes(119), "1 hour before"),
            (TimeDelta::hours(3), "3 hours before"),
            (
                TimeDelta::hours(24) - TimeDelta::minutes(1),
                "23 hours before",
            ),
        ] {
            assert_eq!(
                line(ago),
                format!("release 2026.11.3 in production, {words}")
            );
        }
    }

    /// A change names its kind, place and agent, and says where its day was capped.
    #[test]
    fn a_change_is_named_by_its_kind_its_place_and_its_agent() {
        let ago = TimeDelta::minutes(14);
        let release = released("support", "2026.11.3", ago);
        assert_eq!(
            change_line(&release, opened(), true),
            "support: release 2026.11.3 in production, 14 minutes before"
        );
        let model = Change {
            kind: ChangeKind::Model,
            environment: String::new(),
            value: "gpt-4o-mini".into(),
            capped: true,
            ..release.clone()
        };
        assert_eq!(
            change_line(&model, opened(), false),
            "model gpt-4o-mini, 14 minutes before, and more that day"
        );
        let prompt = Change {
            kind: ChangeKind::Prompt,
            prompt: Some("triage".into()),
            value: "3".into(),
            ..release
        };
        assert_eq!(
            change_line(&prompt, opened(), false),
            "prompt triage version 3 in production, 14 minutes before"
        );
    }

    /// Each name a change carries is clipped, and kept to one line.
    #[test]
    fn a_changes_names_are_clipped() {
        let long = "v".repeat(NAME_SHOWN + 30);
        let change = Change {
            agent: format!("a\n{long}"),
            environment: long.clone(),
            kind: ChangeKind::Prompt,
            prompt: Some(long.clone()),
            ..released("", &long, TimeDelta::minutes(2))
        };
        let line = change_line(&change, opened(), true);
        assert!(!line.contains('\n'), "one line");
        assert!(!line.contains(&"v".repeat(NAME_SHOWN)), "{line}");
        assert_eq!(line.matches('…').count(), 4, "{line}");
        assert!(line.ends_with(", 2 minutes before"));
    }

    /// Seven changes are five lines under their lead and a count of the rest.
    #[test]
    fn a_notice_names_five_changes_and_counts_the_rest() {
        let seven: Vec<Change> = (1..=7)
            .map(|n| released("support", &format!("1.0.{n}"), TimeDelta::minutes(n)))
            .collect();
        let lines = changed_lines(&changed(seven.clone()), BODY_MAX);
        assert_eq!(
            lines,
            [
                "Changed in the 24 hours before it opened:",
                "release 1.0.1 in production, 1 minute before",
                "release 1.0.2 in production, 2 minutes before",
                "release 1.0.3 in production, 3 minutes before",
                "release 1.0.4 in production, 4 minutes before",
                "release 1.0.5 in production, 5 minutes before",
                "and 2 more on the incident",
            ]
        );
        let five = changed_lines(&changed(seven.iter().take(5).cloned().collect()), BODY_MAX);
        assert_eq!(five.len(), 6, "all named, none left to count");
        let over = Changed {
            more: true,
            ..changed(seven)
        };
        assert_eq!(
            changed_lines(&over, BODY_MAX).last().map(String::as_str),
            Some("and over 2 more on the incident"),
            "the incident had more than it names"
        );
    }

    /// With room for fewer than five, fewer are named and counted; with none, none.
    #[test]
    fn changes_that_do_not_fit_are_counted_not_named() {
        let seven: Vec<Change> = (1..=7)
            .map(|n| released("support", &format!("1.0.{n}"), TimeDelta::minutes(n)))
            .collect();
        let all = changed_lines(&changed(seven.clone()), BODY_MAX);
        let cost = |lines: &[String]| lines.iter().map(|line| chars(line) + 1).sum::<usize>();
        assert_eq!(
            changed_lines(&changed(seven.clone()), cost(&all)),
            all,
            "exactly its room"
        );
        let fewer = changed_lines(&changed(seven.clone()), cost(&all) - 1);
        assert_eq!(fewer.len(), 6, "{fewer:?}");
        assert_eq!(
            fewer.last().map(String::as_str),
            Some("and 3 more on the incident")
        );
        assert!(cost(&fewer) < cost(&all));
        assert!(changed_lines(&changed(seven), 60).is_empty());
    }

    /// A body's changes come last and fill it to its bound, one fewer named a character short.
    #[test]
    fn a_body_with_its_changes_stays_inside_its_bound() {
        let long = "é".repeat(NAME_SHOWN + 30);
        let longest: Vec<Change> = (0..7)
            .map(|n| Change {
                agent: long.clone(),
                environment: long.clone(),
                kind: ChangeKind::Prompt,
                prompt: Some(long.clone()),
                capped: true,
                ..released("", &long, TimeDelta::hours(n + 10))
            })
            .collect();
        let several = Changed {
            several: true,
            ..changed(longest)
        };
        let all = changed_lines(&several, BODY_MAX);
        assert_eq!(all.len(), 7, "its lead, five and the count");
        let cost: usize = all.iter().map(|line| chars(line) + 1).sum();
        let traces = format!("Traces: https://console.example/{}", "t".repeat(200));
        // A body whose own three lines leave its changes `room` characters.
        let with_room = |room: usize| {
            let own = BODY_MAX + 1 - room - 3 - chars(&traces);
            let (words, run) = ("w".repeat(own / 2), "r".repeat(own - own / 2));
            let body = carrying(
                words.clone(),
                [Some(run.clone()), Some(traces.clone())],
                Some(&several),
            );
            let lines: Vec<String> = body.lines().map(str::to_owned).collect();
            let kept: Vec<String> = lines.iter().take(3).cloned().collect();
            assert_eq!(kept, [words, run, traces.clone()], "its own lines first");
            assert!(chars(&body) <= BODY_MAX, "{} characters", chars(&body));
            (chars(&body), lines.into_iter().skip(3).collect::<Vec<_>>())
        };
        let (whole, named) = with_room(cost);
        assert_eq!((whole, &named), (BODY_MAX, &all), "exactly full, all named");
        let (_, fewer) = with_room(cost - 1);
        assert_eq!(fewer.len(), 6, "one fewer named: {fewer:?}");
        assert_eq!(
            fewer.last().map(String::as_str),
            Some("and 3 more on the incident"),
            "and counted instead"
        );
        let (whole, none) = with_room(0);
        assert_eq!((whole, none.len()), (BODY_MAX, 0), "a full body names none");
        assert_eq!(
            carrying("w".into(), [Some("t".to_owned())], None),
            "w\nt",
            "and a notice with none is its own lines"
        );
    }

    /// A read that fails or outlasts its budget leaves the notice without changes.
    #[tokio::test]
    async fn changes_that_fail_or_stall_are_gone_without() {
        let id = Uuid::nil();
        let failed = async { Err(TelmoniError::Internal("no store".into())) };
        assert_eq!(
            changed_within(Duration::from_secs(5), id, failed).await,
            None
        );
        let began = std::time::Instant::now();
        let stalled = std::future::pending::<Result<Option<Changed>, TelmoniError>>();
        assert_eq!(
            changed_within(Duration::from_millis(50), id, stalled).await,
            None
        );
        assert!(began.elapsed() < Duration::from_secs(2), "it did not wait");
        let read = changed(vec![released("support", "1.0.1", TimeDelta::minutes(1))]);
        let answered = async { Ok(Some(read.clone())) };
        assert_eq!(
            changed_within(CHANGES_BUDGET, id, answered).await,
            Some(read)
        );
    }
}
