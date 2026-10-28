//! A project's alerts, under [`Resource::Alert`]: `GET` and `POST
//! /internal/v1/telemetry/rules`, and `GET`, `PUT` and `DELETE
//! /rules/{id}`, every one open to any member of the project, as Langfuse
//! lets members make alerts. An alert is its definition, checked whole
//! ([`RuleInput::checked`]), and its last evaluation, which the loop writes
//! ([`crate::rules::evaluate`]). One past its organization's plan, paused
//! ones counted, is refused when it is made: a `402` in the plan's own
//! words. Every change a member makes to one is filed in the organization's
//! audit chain (`chain::file`): an alert decides what the project's
//! connectors hear, and a member's change can silence the owner's.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::AlertCeilings;
use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam;
use telmoni_shared::{
    AuditAction, AuthError, MissingData, ProjectId, RuleAggregation, RuleMeasure, RuleOperator,
    RuleWindow, Severity, SpanKind, TelmoniError, TelmoniResourceKind,
};

use crate::AppState;
use crate::db;
use crate::handler::chain::{self, Change};
use crate::rules::db::{self as rules, LinkRow, RuleRow};
use crate::rules::incidents::{self, IncidentRow};
use crate::rules::{Link, Rule, RuleFilters, RuleInput};

/// The unique index a second alert of one name in a project, in any case,
/// runs into.
const NAME_TAKEN: &str = "rules_project_name_idx";

/// One alert, as the lanes answer it.
#[derive(Debug, Serialize)]
pub struct Alert {
    pub id: Uuid,
    pub name: String,
    pub kind: SpanKind,
    pub measure: RuleMeasure,
    pub aggregation: RuleAggregation,
    pub filters: RuleFilters,
    pub window: RuleWindow,
    pub operator: RuleOperator,
    pub alert_threshold: f64,
    pub warning_threshold: Option<f64>,
    pub missing_data: MissingData,
    pub renotify_minutes: Option<i32>,
    /// Who paused it; none while it runs.
    pub paused: Option<crate::rules::db::PausedBy>,
    pub paused_at: Option<DateTime<Utc>>,
    /// Its value at its last evaluation: none before the first, or where
    /// the window held no span its filters admit.
    pub value: Option<f64>,
    pub evaluated_at: Option<DateTime<Utc>>,
    /// Where it stands, and since when: `paused` while it is.
    pub severity: Severity,
    pub severity_since: DateTime<Utc>,
    /// The connectors its notices go to.
    pub connectors: Vec<AlertLink>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A connector an alert links, and whether five failed deliveries in a row
/// muted it for this alert.
#[derive(Debug, Serialize)]
pub struct AlertLink {
    #[serde(flatten)]
    pub link: Link,
    pub muted: bool,
}

impl Alert {
    /// The alert as its row, its links and its muted connectors give it.
    fn of(row: RuleRow, links: Vec<Link>, muted: &[Uuid]) -> Self {
        let connectors = links
            .into_iter()
            .map(|link| AlertLink {
                muted: muted.contains(&link.connection_id),
                link,
            })
            .collect();
        Self {
            connectors,
            ..Self::from(row)
        }
    }
}

impl From<RuleRow> for Alert {
    fn from(row: RuleRow) -> Self {
        Self {
            connectors: Vec::new(),
            id: row.id,
            name: row.name,
            kind: row.kind,
            measure: row.measure,
            aggregation: row.aggregation,
            filters: row.filters.0,
            window: row.lookback,
            operator: row.operator,
            alert_threshold: row.alert_threshold,
            warning_threshold: row.warning_threshold,
            missing_data: row.missing_data,
            renotify_minutes: row.renotify_minutes,
            paused: row.paused,
            paused_at: row.paused_at,
            value: row.value,
            evaluated_at: row.evaluated_at,
            severity: row.severity,
            severity_since: row.severity_since,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// What the list of alerts answers.
#[derive(Debug, Serialize)]
pub struct AlertList {
    /// The project's alerts, by name.
    pub rules: Vec<Alert>,
}

/// An alert's id from its path: a UUID, or an alert the project does not have.
fn rule_id(given: &str) -> Result<Uuid, TelmoniError> {
    given.parse().map_err(|_| not_found())
}

fn not_found() -> TelmoniError {
    AuthError::NotFound("no alert with that id in this project".into()).into()
}

/// A write the unique index refused: the project has an alert by that name.
/// Any other failure is the database's.
fn name_taken(error: sqlx::Error) -> TelmoniError {
    let taken = error
        .as_database_error()
        .and_then(|db| db.constraint())
        .is_some_and(|constraint| constraint == NAME_TAKEN);
    if taken {
        return AuthError::Conflict("this project has an alert by that name already".into()).into();
    }
    error.into()
}

/// `GET /internal/v1/telemetry/rules` — the project's alerts, by name.
pub async fn list_rules(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<AlertList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Alert)?
        .project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = rules::rules(&mut tx, project_id).await?;
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut links = rules::links(&mut tx, project_id, &ids).await?;
    tx.commit().await?;
    let mut alerts = Vec::with_capacity(rows.len());
    for row in rows {
        let (own, rest): (Vec<_>, Vec<_>) = links.into_iter().partition(|l| l.rule_id == row.id);
        links = rest;
        let muted = muted(&state, project_id, row.id).await;
        alerts.push(Alert::of(
            row,
            own.into_iter().map(LinkRow::link).collect(),
            &muted,
        ));
    }
    Ok(Json(AlertList { rules: alerts }))
}

/// `GET /internal/v1/telemetry/rules/{id}` — one of the project's alerts.
pub async fn get_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Alert>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Alert)?
        .project_id;
    let id = rule_id(&id)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let row = rules::rule(&mut tx, project_id, id)
        .await?
        .ok_or_else(not_found)?;
    let links = rules::links(&mut tx, project_id, &[id]).await?;
    tx.commit().await?;
    let muted = muted(&state, project_id, id).await;
    Ok(Json(Alert::of(
        row,
        links.into_iter().map(LinkRow::link).collect(),
        &muted,
    )))
}

/// `POST /internal/v1/telemetry/rules` — a new alert of the project. Its
/// plan's number is read first, outside any lock, and held to under the
/// organization's chain lock, so two alerts made at once in two of its
/// projects never both take its last place; the project's settings row,
/// which the purges find its alerts by, is made with it where there is none.
pub async fn create_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<RuleInput>,
) -> Result<(StatusCode, Json<Alert>), TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Create, Resource::Alert)?
        .project_id
        .clone();
    let rule = input.checked()?;
    let ceilings = seam::alert_ceilings(state.entitlements.as_ref(), &acting.organization_id).await;
    let mut tx = chain::scoped(&state.db, &acting, &project_id).await?;
    let row = match save(&state, &acting, &headers, &mut tx, &rule, ceilings.as_ref()).await {
        Ok(row) => row,
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, &project_id, "alert");
            return Err(refused);
        }
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, rule_id = %row.id, "telemetry: an alert made");
    Ok((
        StatusCode::CREATED,
        Json(Alert::of(row, rule.connectors, &[])),
    ))
}

/// The save itself, under the organization's chain lock: the role checked
/// on auth's second answer, the settings row made or re-filed, the
/// organization's alerts counted against its plan, and the alert written
/// and filed.
async fn save(
    state: &AppState,
    acting: &Acting,
    headers: &HeaderMap,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    rule: &Rule,
    ceilings: Option<&AlertCeilings>,
) -> Result<RuleRow, TelmoniError> {
    let project_id = &held(state, acting, tx, Verb::Create).await?;
    db::ensure_settings(tx, project_id, &acting.organization_id).await?;
    if let Some(ceilings) = ceilings {
        let held = rules::organization_count(tx, &acting.organization_id).await?;
        ceilings.admit(u64::try_from(held).unwrap_or(0))?;
    }
    let row = rules::insert(
        tx,
        project_id,
        &acting.organization_id,
        Uuid::new_v4(),
        rule,
    )
    .await
    .map_err(name_taken)?;
    rules::write_links(tx, project_id, row.id, &rule.connectors).await?;
    let change = Change {
        action: AuditAction::Created,
        kind: TelmoniResourceKind::Alert,
        id: row.id,
        metadata: json!({ "name": row.name }),
    };
    chain::file(tx, acting, project_id, headers, change).await?;
    Ok(row)
}

/// The organization's chain lock taken in `tx`, and `verb` on the project's
/// alerts checked on auth's answer under it: the project acted on.
async fn held(
    state: &AppState,
    acting: &Acting,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    verb: Verb,
) -> Result<ProjectId, TelmoniError> {
    let project_id = acting
        .require_project(verb, Resource::Alert)?
        .project_id
        .clone();
    let now = chain::hold(state.auth.as_ref(), acting, tx).await?;
    now.require_project(verb, Resource::Alert)?;
    Ok(project_id)
}

/// `write` run in a transaction bound to the project and its organization:
/// committed where it answers, rolled back at once where it is refused, so
/// its chain lock goes before the answer.
async fn under_chain<T>(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    write: impl AsyncFnOnce(&mut Scoped<'_, ProjectAndOrganization>) -> Result<T, TelmoniError>,
) -> Result<T, TelmoniError> {
    let mut tx = chain::scoped(&state.db, acting, project_id).await?;
    match write(&mut tx).await {
        Ok(done) => {
            tx.commit().await?;
            Ok(done)
        }
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, project_id, "alert");
            Err(refused)
        }
    }
}

/// `PUT /internal/v1/telemetry/rules/{id}` — one of the project's alerts,
/// saved over whole; its last evaluation is forgotten, since it answered
/// another question, and the loop evaluates it again within a minute.
pub async fn update_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<RuleInput>,
) -> Result<Json<Alert>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    let id = rule_id(&id)?;
    let rule = input.checked()?;
    let row = under_chain(&state, &acting, project_id, async |tx| {
        held(&state, &acting, tx, Verb::Update).await?;
        let before = rules::rule(tx, project_id, id)
            .await?
            .ok_or_else(not_found)?;
        let linked: Vec<Link> = rules::links(tx, project_id, &[id])
            .await?
            .into_iter()
            .map(LinkRow::link)
            .collect();
        let row = rules::update(tx, project_id, id, &rule)
            .await
            .map_err(name_taken)?
            .ok_or_else(not_found)?;
        rules::write_links(tx, project_id, id, &rule.connectors).await?;
        let change = Change {
            action: AuditAction::Updated,
            kind: TelmoniResourceKind::Alert,
            id,
            metadata: json!({ "name": row.name, "changed": changed(&before, &linked, &rule) }),
        };
        chain::file(tx, &acting, project_id, &headers, change).await?;
        Ok(row)
    })
    .await?;
    tracing::info!(project_id = %project_id, rule_id = %id, "telemetry: an alert saved over");
    let muted = muted(&state, project_id, id).await;
    Ok(Json(Alert::of(row, rule.connectors, &muted)))
}

/// The fields a save over changes, by the names the lane takes them under.
fn changed(before: &RuleRow, linked: &[Link], rule: &Rule) -> Vec<&'static str> {
    let bits = |value: Option<f64>| value.map(f64::to_bits);
    let relinked = linked.len() != rule.connectors.len()
        || !linked.iter().all(|link| rule.connectors.contains(link));
    [
        ("name", before.name != rule.name),
        ("kind", before.kind != rule.kind),
        ("measure", before.measure != rule.measure),
        ("aggregation", before.aggregation != rule.aggregation),
        ("filters", before.filters.0 != rule.filters),
        ("window", before.lookback != rule.window),
        ("operator", before.operator != rule.operator),
        (
            "alert_threshold",
            bits(Some(before.alert_threshold)) != bits(Some(rule.alert_threshold)),
        ),
        (
            "warning_threshold",
            bits(before.warning_threshold) != bits(rule.warning_threshold),
        ),
        ("missing_data", before.missing_data != rule.missing_data),
        (
            "renotify_minutes",
            before.renotify_minutes != rule.renotify_minutes,
        ),
        ("connectors", relinked),
    ]
    .into_iter()
    .filter_map(|(field, moved)| moved.then_some(field))
    .collect()
}

/// `DELETE /internal/v1/telemetry/rules/{id}` — one of the project's
/// alerts, gone, with its incidents and windows; its open incident is told
/// resolved, as a member's resolution is, to the connectors it linked.
pub async fn delete_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Delete, Resource::Alert)?
        .project_id;
    let id = rule_id(&id)?;
    let (open, links): (Option<IncidentRow>, Vec<Link>) =
        under_chain(&state, &acting, project_id, async |tx| {
            held(&state, &acting, tx, Verb::Delete).await?;
            let row = rules::rule(tx, project_id, id)
                .await?
                .ok_or_else(not_found)?;
            let open = incidents::open_of(tx, id).await?;
            let links = rules::links(tx, project_id, &[id])
                .await?
                .into_iter()
                .map(LinkRow::link)
                .collect();
            if !rules::delete(tx, project_id, id).await? {
                return Err(not_found());
            }
            let change = Change {
                action: AuditAction::Deleted,
                kind: TelmoniResourceKind::Alert,
                id,
                metadata: json!({ "name": row.name }),
            };
            chain::file(tx, &acting, project_id, &headers, change).await?;
            Ok((open, links))
        })
        .await?;
    tracing::info!(project_id = %project_id, rule_id = %id, "telemetry: an alert deleted");
    if let Some(open) = open {
        crate::notify::deleted_with_alert(&state, &open, links).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The connectors five failed deliveries in a row muted for the alert; none,
/// logged, where notifications cannot say.
async fn muted(state: &AppState, project_id: &ProjectId, rule_id: Uuid) -> Vec<Uuid> {
    match state
        .notifications
        .muted(project_id, &rules::route_key(rule_id))
        .await
    {
        Ok(muted) => muted,
        Err(error) => {
            tracing::warn!(
                project_id = %project_id,
                rule_id = %rule_id,
                error = %error,
                "telemetry: which of an alert's connectors are muted could not be read"
            );
            Vec::new()
        }
    }
}

/// `POST /internal/v1/telemetry/rules/{id}/connectors/{connection_id}/unmute`
/// — a connector the alert links, muted after five failed deliveries in a
/// row, hearing the alert's notices again, its failures forgotten. Answers
/// the alert.
pub async fn unmute_link(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((id, connection_id)): Path<(String, String)>,
) -> Result<Json<Alert>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    let id = rule_id(&id)?;
    let connection_id: Uuid = connection_id.parse().map_err(|_| not_linked())?;
    let (row, links) = under_chain(&state, &acting, project_id, async |tx| {
        held(&state, &acting, tx, Verb::Update).await?;
        let row = rules::rule(tx, project_id, id)
            .await?
            .ok_or_else(not_found)?;
        let links: Vec<Link> = rules::links(tx, project_id, &[id])
            .await?
            .into_iter()
            .map(LinkRow::link)
            .collect();
        if !links.iter().any(|link| link.connection_id == connection_id) {
            return Err(not_linked());
        }
        if state
            .notifications
            .unmute(project_id, &rules::route_key(id), connection_id)
            .await?
        {
            tracing::info!(project_id = %project_id, rule_id = %id, connection_id = %connection_id, "telemetry: an alert's connector heard again");
            let change = Change {
                action: AuditAction::Updated,
                kind: TelmoniResourceKind::Alert,
                id,
                metadata: json!({ "name": row.name, "unmuted": connection_id }),
            };
            chain::file(tx, &acting, project_id, &headers, change).await?;
        }
        Ok((row, links))
    })
    .await?;
    let muted = muted(&state, project_id, id).await;
    Ok(Json(Alert::of(row, links, &muted)))
}

fn not_linked() -> TelmoniError {
    AuthError::NotFound("this alert links no such connector".into()).into()
}

/// `POST /internal/v1/telemetry/rules/{id}/pause` — one of the project's
/// alerts paused by the person: it is not evaluated, and tells nothing, until
/// it is resumed; still counted against the plan. One paused already is
/// answered as it stands.
pub async fn pause_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Alert>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    let id = rule_id(&id)?;
    let (row, links) = under_chain(&state, &acting, project_id, async |tx| {
        held(&state, &acting, tx, Verb::Update).await?;
        let before = rules::rule(tx, project_id, id)
            .await?
            .ok_or_else(not_found)?;
        let row = rules::pause(tx, project_id, id)
            .await?
            .ok_or_else(not_found)?;
        if before.paused != row.paused {
            let change = Change {
                action: AuditAction::Updated,
                kind: TelmoniResourceKind::Alert,
                id,
                metadata: json!({
                    "name": row.name,
                    "paused": { "from": before.paused, "to": row.paused },
                }),
            };
            chain::file(tx, &acting, project_id, &headers, change).await?;
        }
        Ok((row, rules::links(tx, project_id, &[id]).await?))
    })
    .await?;
    tracing::info!(project_id = %project_id, rule_id = %id, "telemetry: an alert paused");
    let muted = muted(&state, project_id, id).await;
    Ok(Json(Alert::of(
        row,
        links.into_iter().map(LinkRow::link).collect(),
        &muted,
    )))
}

/// `POST /internal/v1/telemetry/rules/{id}/resume` — one of the project's
/// paused alerts running again within a minute. One its plan holds no room
/// for, counting the organization's older alerts, paused or not, is refused
/// with the `402` that says what makes room.
pub async fn resume_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Alert>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    let id = rule_id(&id)?;
    let ceilings = seam::alert_ceilings(state.entitlements.as_ref(), &acting.organization_id).await;
    let (row, links) = under_chain(&state, &acting, project_id, async |tx| {
        held(&state, &acting, tx, Verb::Update).await?;
        let before = rules::rule(tx, project_id, id)
            .await?
            .ok_or_else(not_found)?;
        if let Some(ceilings) = &ceilings {
            let ahead = rules::organization_ahead(tx, &acting.organization_id, id).await?;
            ceilings.admit(u64::try_from(ahead).unwrap_or(0))?;
        }
        let row = rules::resume(tx, project_id, id)
            .await?
            .ok_or_else(not_found)?;
        if before.paused != row.paused {
            let change = Change {
                action: AuditAction::Updated,
                kind: TelmoniResourceKind::Alert,
                id,
                metadata: json!({
                    "name": row.name,
                    "paused": { "from": before.paused, "to": row.paused },
                }),
            };
            chain::file(tx, &acting, project_id, &headers, change).await?;
        }
        Ok((row, rules::links(tx, project_id, &[id]).await?))
    })
    .await?;
    tracing::info!(project_id = %project_id, rule_id = %id, "telemetry: an alert resumed");
    let muted = muted(&state, project_id, id).await;
    Ok(Json(Alert::of(
        row,
        links.into_iter().map(LinkRow::link).collect(),
        &muted,
    )))
}
