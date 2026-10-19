//! A project's saved views, under [`Resource::View`]: views of its trace
//! list (`kind` = `list`) and layouts of a session's page and a trace's
//! (`layout`). `GET /internal/v1/telemetry/views?kind=`, which every role on
//! the project reads, and `POST`, `PUT /views/{id}` and `DELETE /views/{id}`,
//! which its owners and admins send. A view is a name, a query checked as
//! its page would read it — the list's filters and sort, or a layout's
//! options — and the columns shown, so a view always opens. No change to a
//! view is audited: a view changes what a page opens on, never what anyone
//! may see or what a project keeps.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam::Auth;
use telmoni_shared::{
    AuthError, LayoutDepth, LayoutOpen, ProjectId, SortDirection, SpanKind, SpanStatus,
    TelmoniError, TraceColumn, TraceSort, ViewKind,
};

use crate::AppState;
use crate::contract::NAME_MAX;
use crate::db::{self, ViewRow, ViewWrite};
use crate::handler::chain;
use crate::handler::traces::{TraceQuery, parse_limit, values};
use crate::traces::PINNED_MAX;

/// The most views of one kind a project keeps: more than a menu shows at a
/// glance.
pub const VIEWS_MAX: i64 = 100;
/// The longest a view's name is, in characters, as the table's CHECK has it.
pub const VIEW_NAME_MAX: usize = 60;
/// The unique index a second view of one kind and name, in any case, runs
/// into.
const NAME_TAKEN: &str = "trace_views_project_name_idx";

/// The list's query string: which kind of view, `list` when left out.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewsQuery {
    #[serde(default)]
    pub kind: String,
}

/// `kind` as a URL carries it: `list` when empty.
fn parse_kind(text: &str) -> Result<ViewKind, TelmoniError> {
    match text.trim() {
        "" => Ok(ViewKind::List),
        given => given
            .parse()
            .map_err(|_| TelmoniError::BadRequest("kind is list or layout".into())),
    }
}

/// A list view's query: the list's, each filter's values a list. A key left
/// out narrows nothing, or asks for the list's default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ViewQuery {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agent: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub user: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub session: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<SpanStatus>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub model: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub release: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tag: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prompt: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<TraceSort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<SortDirection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl ViewQuery {
    /// The query checked by the list's own rules, and kept as they leave it:
    /// each value trimmed and given once.
    fn checked(self) -> Result<Self, TelmoniError> {
        let query = TraceQuery {
            agent: self.agent,
            user: self.user,
            session: self.session,
            status: self.status.iter().map(ToString::to_string).collect(),
            model: self.model,
            tool: self.tool,
            environment: self.environment,
            release: self.release,
            tag: self.tag,
            prompt: self.prompt,
            days: self.days.map(|days| days.to_string()),
            ..TraceQuery::default()
        };
        let filter = query.filter()?;
        if let Some(limit) = self.limit {
            parse_limit(&limit.to_string())?;
        }
        Ok(Self {
            agent: filter.agents,
            user: filter.user_ids,
            session: filter.session_ids,
            status: filter.statuses,
            model: filter.models,
            tool: filter.tools,
            environment: filter.environments,
            release: filter.releases,
            tag: filter.tags,
            prompt: filter.prompts,
            sort: self.sort,
            dir: self.dir,
            days: self.days,
            limit: self.limit,
        })
    }
}

/// The calls a layout shows: by kind, by how they ended and by name, any
/// value of each admitting a call, and an empty one every call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutFilter {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kind: Vec<SpanKind>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<SpanStatus>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub name: Vec<String>,
}

impl LayoutFilter {
    fn checked(self) -> Result<Self, TelmoniError> {
        Ok(Self {
            kind: once(self.kind),
            status: once(self.status),
            name: values("name", &self.name, NAME_MAX)?,
        })
    }
}

/// `given`, each value once, in the order first given.
fn once<T: PartialEq>(given: Vec<T>) -> Vec<T> {
    let mut kept: Vec<T> = Vec::new();
    for value in given {
        if !kept.contains(&value) {
            kept.push(value);
        }
    }
    kept
}

/// A layout of a session's page and a trace's: what a saved layout's `query`
/// holds, its columns in `columns` beside it. A key left out is the plainest
/// choice, so a layout saved by an older console still reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutOptions {
    /// How far a trace opens.
    pub depth: LayoutDepth,
    /// Which traces, and which branches of them, start open.
    pub open: LayoutOpen,
    /// Each span's bar on its trace's own line of time.
    pub timeline: bool,
    /// Span names whose spans show under each trace while it is shut.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<String>,
    /// The calls shown.
    pub filter: LayoutFilter,
    /// The run's own input and output, once a project keeps content.
    pub messages: bool,
    /// The trace's scores, once scored.
    pub scores: bool,
}

impl LayoutOptions {
    /// The options checked as the page would read them: each pinned name
    /// trimmed and once, and at most [`PINNED_MAX`] of them.
    fn checked(self) -> Result<Self, TelmoniError> {
        let pinned = values("pinned", &self.pinned, NAME_MAX)?;
        if pinned.len() > PINNED_MAX {
            return Err(TelmoniError::BadRequest(format!(
                "a layout pins at most {PINNED_MAX} span names"
            )));
        }
        Ok(Self {
            pinned,
            filter: self.filter.checked()?,
            ..self
        })
    }
}

/// What saving a view sends: its name, its kind, its query — the list's, or
/// a layout's options — and the columns shown. The trace's id is always
/// shown, and is none of the columns.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewInput {
    pub name: String,
    #[serde(default)]
    pub kind: ViewKind,
    #[serde(default)]
    pub query: Value,
    #[serde(default)]
    pub columns: Vec<TraceColumn>,
}

/// A view as it is written: its name trimmed, its query checked and as JSON,
/// its columns each once, in the list's order.
#[derive(Debug, PartialEq)]
struct Checked {
    name: String,
    kind: ViewKind,
    query: Value,
    columns: Vec<String>,
}

/// `value` as JSON, which these shapes always are.
fn written<T: Serialize>(value: &T) -> Result<Value, TelmoniError> {
    serde_json::to_value(value)
        .map_err(|e| TelmoniError::Internal(format!("a view's query could not be written: {e}")))
}

impl ViewInput {
    fn checked(self) -> Result<Checked, TelmoniError> {
        let name = self.name.trim();
        if name.is_empty()
            || name.chars().count() > VIEW_NAME_MAX
            || name.chars().any(char::is_control)
        {
            return Err(TelmoniError::BadRequest(format!(
                "a view's name is 1 to {VIEW_NAME_MAX} characters, none of them a control \
                 character"
            )));
        }
        let given = if self.query.is_null() {
            json!({})
        } else {
            self.query
        };
        // A key the page does not read is refused as it arrives, and never
        // echoed: a person typed it, or an older console sent it.
        let query = match self.kind {
            ViewKind::List => written(
                &serde_json::from_value::<ViewQuery>(given)
                    .map_err(|_| {
                        TelmoniError::BadRequest(
                            "a view's query names a filter the list does not take".into(),
                        )
                    })?
                    .checked()?,
            )?,
            ViewKind::Layout => written(
                &serde_json::from_value::<LayoutOptions>(given)
                    .map_err(|_| {
                        TelmoniError::BadRequest(
                            "a layout's options are not ones the page reads".into(),
                        )
                    })?
                    .checked()?,
            )?,
        };
        let columns = TraceColumn::all()
            .into_iter()
            .filter(|column| self.columns.contains(column))
            .map(|column| column.as_str().to_owned())
            .collect();
        Ok(Checked {
            name: name.to_owned(),
            kind: self.kind,
            query,
            columns,
        })
    }
}

impl Checked {
    fn write(&self) -> ViewWrite<'_> {
        ViewWrite {
            name: &self.name,
            query: &self.query,
            columns: &self.columns,
        }
    }
}

/// One saved view, as the lanes answer it: its query the list's, or a
/// layout's options, as its kind says.
#[derive(Debug, Serialize)]
pub struct View {
    pub id: Uuid,
    pub kind: ViewKind,
    pub name: String,
    pub query: Value,
    pub columns: Vec<TraceColumn>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A row's query as its kind's page reads it. One no longer read — written
/// before a filter was renamed — is answered as the kind's plainest, so the
/// view opens on the whole list, or the plainest layout, rather than taking
/// the menu down with it.
fn readable(kind: ViewKind, id: Uuid, query: Value) -> Value {
    let read = match kind {
        ViewKind::List => serde_json::from_value::<ViewQuery>(query).map(|q| written(&q)),
        ViewKind::Layout => serde_json::from_value::<LayoutOptions>(query).map(|o| written(&o)),
    };
    match read {
        Ok(Ok(value)) => value,
        _ => {
            tracing::warn!(view_id = %id, kind = %kind, "telemetry: a saved view's query no longer reads");
            match kind {
                ViewKind::List => json!({}),
                ViewKind::Layout => {
                    written(&LayoutOptions::default()).unwrap_or_else(|_| json!({}))
                }
            }
        }
    }
}

impl From<ViewRow> for View {
    /// A column no longer the list's is left out.
    fn from(row: ViewRow) -> Self {
        Self {
            id: row.id,
            kind: row.kind,
            name: row.name,
            query: readable(row.kind, row.id, row.query),
            columns: row
                .columns
                .iter()
                .filter_map(|column| column.parse().ok())
                .collect(),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// What the list of views answers.
#[derive(Debug, Serialize)]
pub struct ViewList {
    /// The project's views of the kind asked for, by name.
    pub views: Vec<View>,
}

/// A view's id from its path: a UUID, or a view the project does not have.
fn view_id(given: &str) -> Result<Uuid, TelmoniError> {
    given
        .parse()
        .map_err(|_| AuthError::NotFound("no view with that id in this project".into()).into())
}

/// What a kind's views are called, in a refusal.
const fn what(kind: ViewKind) -> &'static str {
    match kind {
        ViewKind::List => "view",
        ViewKind::Layout => "layout",
    }
}

/// A write the unique index refused: a view of that kind and name is the
/// project's already. Any other failure is the database's.
fn name_taken(kind: ViewKind) -> impl Fn(sqlx::Error) -> TelmoniError {
    move |error| {
        let taken = error
            .as_database_error()
            .and_then(|db| db.constraint())
            .is_some_and(|constraint| constraint == NAME_TAKEN);
        if taken {
            return AuthError::Conflict(format!(
                "this project has a {} by that name already",
                what(kind)
            ))
            .into();
        }
        error.into()
    }
}

/// `GET /internal/v1/telemetry/views` — the project's views of one kind, by
/// name: the list's unless `kind` says `layout`.
pub async fn list_views(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ViewsQuery>,
) -> Result<Json<ViewList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::View)?
        .project_id;
    let kind = parse_kind(&query.kind)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = db::views(&mut tx, project_id, kind).await?;
    tx.commit().await?;
    Ok(Json(ViewList {
        views: rows.into_iter().map(View::from).collect(),
    }))
}

/// `POST /internal/v1/telemetry/views` — a new view of the project. Written
/// under the organization's chain lock, as a content mode is: the project's
/// settings row is made with it where the project has none — the row the
/// organization's purge finds the project's views by — and must be filed
/// under the organization that holds the project now. That row's lock, held
/// to the commit, also puts two saves on one project one after the other,
/// so neither counts past [`VIEWS_MAX`].
pub async fn create_view(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<ViewInput>,
) -> Result<(StatusCode, Json<View>), TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    // Refused before any lock is taken, so nobody who may not save waits on
    // the organization's chain to be told no.
    let project_id = acting
        .require_project(Verb::Create, Resource::View)?
        .project_id
        .clone();
    let view = input.checked()?;
    let mut tx = chain::scoped(&state.db, &acting, &project_id).await?;
    let row = match save(&*state.auth, &acting, &project_id, &mut tx, &view).await {
        Ok(row) => row,
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, &project_id, "view");
            return Err(refused);
        }
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, view_id = %row.id, kind = %row.kind, "telemetry: a view saved");
    Ok((StatusCode::CREATED, Json(View::from(row))))
}

/// The save itself, inside the transaction [`create_view`] opened: under the
/// organization's chain lock, the role checked again on auth's second answer,
/// the project's settings row made or re-filed, and the view written where
/// the project keeps fewer than [`VIEWS_MAX`] of its kind.
async fn save(
    auth: &dyn Auth,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    view: &Checked,
) -> Result<ViewRow, TelmoniError> {
    let now = chain::hold(auth, acting, project_id, tx).await?;
    now.require_project(Verb::Create, Resource::View)?;
    db::ensure_settings(tx, project_id, &acting.organization_id).await?;
    if db::view_count(tx, project_id, view.kind).await? >= VIEWS_MAX {
        return Err(AuthError::Conflict(format!(
            "a project keeps at most {VIEWS_MAX} {}s; delete one to save another",
            what(view.kind)
        ))
        .into());
    }
    db::insert_view(tx, project_id, Uuid::new_v4(), view.kind, view.write())
        .await
        .map_err(name_taken(view.kind))
}

/// `PUT /internal/v1/telemetry/views/{id}` — one of the project's views, saved
/// over: its name, its query and its columns; never its kind, which the body
/// names so the query is checked as that kind's page reads it. Its project's
/// settings row is there already, made when it was first saved.
pub async fn update_view(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ViewInput>,
) -> Result<Json<View>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::View)?
        .project_id;
    let id = view_id(&id)?;
    let view = input.checked()?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let Some(row) = db::update_view(&mut tx, project_id, id, view.kind, view.write())
        .await
        .map_err(name_taken(view.kind))?
    else {
        return Err(AuthError::NotFound(format!(
            "no {} with that id in this project",
            what(view.kind)
        ))
        .into());
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, view_id = %id, "telemetry: a view saved over");
    Ok(Json(View::from(row)))
}

/// `DELETE /internal/v1/telemetry/views/{id}` — one of the project's views,
/// gone; a layout the project opened in by default takes the default with
/// it, and the project opens in the default layout again (the key on
/// `project_settings.session_view_id`).
pub async fn delete_view(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Delete, Resource::View)?
        .project_id;
    let id = view_id(&id)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    if !db::delete_view(&mut tx, project_id, id).await? {
        return Err(AuthError::NotFound("no view with that id in this project".into()).into());
    }
    tx.commit().await?;
    tracing::info!(project_id = %project_id, view_id = %id, "telemetry: a view deleted");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, query: Value, columns: &[&str]) -> ViewInput {
        serde_json::from_value(json!({
            "name": name,
            "query": query,
            "columns": columns,
        }))
        .expect("a view's body")
    }

    fn layout(name: &str, query: Value, columns: &[&str]) -> ViewInput {
        serde_json::from_value(json!({
            "name": name,
            "kind": "layout",
            "query": query,
            "columns": columns,
        }))
        .expect("a layout's body")
    }

    /// A view's name is trimmed, its query kept as the list reads it — each
    /// value trimmed and once — and its columns each once, in the list's
    /// order.
    #[test]
    fn a_view_is_kept_as_the_list_would_read_it() {
        let checked = input(
            "  Errors this week ",
            json!({
                "agent": [" support-bot ", "support-bot", ""],
                "status": ["error"],
                "environment": ["production"],
                "tag": [" team:search "],
                "sort": "tokens",
                "dir": "asc",
                "days": 7,
                "limit": 50,
            }),
            &["duration", "started", "duration", "status"],
        )
        .checked()
        .expect("a view");
        assert_eq!(checked.name, "Errors this week");
        assert_eq!(checked.kind, ViewKind::List);
        assert_eq!(
            checked.query,
            json!({
                "agent": ["support-bot"],
                "status": ["error"],
                "environment": ["production"],
                "tag": ["team:search"],
                "sort": "tokens",
                "dir": "asc",
                "days": 7,
                "limit": 50,
            })
        );
        assert_eq!(checked.columns, ["started", "status", "duration"]);
        let empty = input("All", json!({}), &[])
            .checked()
            .expect("a view of the whole list");
        assert_eq!(empty.query, json!({}));
    }

    /// A layout's options are kept as the page reads them: each pinned name
    /// and filtered name trimmed and once, a kind once, and a key left out
    /// the plainest choice.
    #[test]
    fn a_layout_is_kept_as_the_page_would_read_it() {
        let checked = layout(
            " Review ",
            json!({
                "depth": "calls",
                "open": "failed",
                "timeline": true,
                "pinned": [" plan ", "plan", "", "search_docs"],
                "filter": { "kind": ["tool_call", "tool_call"], "status": ["error"], "name": [" search_docs "] },
                "messages": true,
            }),
            &["tokens", "started", "error_type"],
        )
        .checked()
        .expect("a layout");
        assert_eq!(checked.name, "Review");
        assert_eq!(checked.kind, ViewKind::Layout);
        assert_eq!(
            checked.query,
            json!({
                "depth": "calls",
                "open": "failed",
                "timeline": true,
                "pinned": ["plan", "search_docs"],
                "filter": { "kind": ["tool_call"], "status": ["error"], "name": ["search_docs"] },
                "messages": true,
                "scores": false,
            })
        );
        assert_eq!(checked.columns, ["started", "error_type", "tokens"]);
        let plain = layout("Plain", json!({}), &[])
            .checked()
            .expect("the plainest");
        assert_eq!(
            plain.query,
            json!({ "depth": "trace", "open": "none", "timeline": false, "filter": {}, "messages": false, "scores": false })
        );
    }

    /// ⚠ A name or a query the list would refuse is a 400 that echoes none
    /// of it, as is a query naming a filter there is none of, a kind that is
    /// neither, or a column the list hides none of.
    #[test]
    fn a_view_the_list_would_refuse_is_refused() {
        for (name, query) in [
            ("", json!({})),
            ("   ", json!({})),
            (&*"v".repeat(VIEW_NAME_MAX + 1), json!({})),
            ("two\nlines<script>", json!({})),
            ("Late", json!({ "days": 0 })),
            ("Long", json!({ "days": 361 })),
            ("Big", json!({ "limit": 501 })),
            ("Agent", json!({ "agent": ["a".repeat(201)] })),
            ("Tag", json!({ "tag": ["untagged<script>"] })),
            ("Spend", json!({ "spend<script>": ["high"] })),
            ("Depth", json!({ "depth": "calls" })),
        ] {
            let refused = input(name, query, &[]).checked().expect_err(name);
            let problem = refused.to_problem_details();
            assert_eq!(problem.status, 400, "{name}");
            assert!(!problem.detail.unwrap_or_default().contains("script"));
        }
        let kind = serde_json::from_value::<ViewInput>(json!({ "name": "x", "kind": "page" }));
        assert!(kind.is_err(), "a kind there is none of");
        let column = serde_json::from_value::<ViewInput>(json!({
            "name": "x",
            "columns": ["run_id"],
        }));
        assert!(column.is_err(), "a column the list hides none of");
    }

    /// ⚠ A layout the page would refuse is a 400 that echoes none of it: a
    /// depth or an opening there is none of, a list's filter, a kind of span
    /// there is none of, or more pinned names than a trace shows.
    #[test]
    fn a_layout_the_page_would_refuse_is_refused() {
        let nine: Vec<String> = (0..=PINNED_MAX).map(|i| format!("step-{i}")).collect();
        for (name, query) in [
            ("Depth", json!({ "depth": "spans<script>" })),
            ("Open", json!({ "open": "errors<script>" })),
            ("List", json!({ "agent": ["support-bot"] })),
            ("Kind", json!({ "filter": { "kind": ["call<script>"] } })),
            ("Pins", json!({ "pinned": nine })),
            ("Pin", json!({ "pinned": ["p".repeat(NAME_MAX + 1)] })),
        ] {
            let refused = layout(name, query, &[]).checked().expect_err(name);
            let problem = refused.to_problem_details();
            assert_eq!(problem.status, 400, "{name}");
            assert!(!problem.detail.unwrap_or_default().contains("script"));
        }
    }

    /// A view whose stored query no longer reads opens on the whole list, a
    /// layout's on the plainest layout, and a column no longer the list's is
    /// left out.
    #[test]
    fn a_row_that_no_longer_reads_opens_on_the_plainest() {
        let row = |kind, query| ViewRow {
            id: Uuid::nil(),
            kind,
            name: "Old".into(),
            query,
            columns: vec!["started".into(), "spend".into()],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let view = View::from(row(ViewKind::List, json!({ "customer": ["c-1"] })));
        assert_eq!(view.query, json!({}));
        assert_eq!(view.columns, [TraceColumn::Started]);
        let layout = View::from(row(ViewKind::Layout, json!({ "depth": "spans" })));
        assert_eq!(layout.query["depth"], "trace");
        assert_eq!(layout.query["open"], "none");
        let kept = View::from(row(
            ViewKind::Layout,
            json!({ "depth": "steps", "pinned": ["plan"] }),
        ));
        assert_eq!(kept.query["depth"], "steps");
        assert_eq!(kept.query["pinned"], json!(["plan"]));
    }

    #[test]
    fn the_kind_is_the_lists_unless_the_url_says_layout() {
        assert_eq!(parse_kind("").expect("the default"), ViewKind::List);
        assert_eq!(parse_kind(" layout ").expect("a layout"), ViewKind::Layout);
        let refused = parse_kind("page<script>")
            .expect_err("refused")
            .to_problem_details();
        assert_eq!(refused.status, 400);
        assert!(!refused.detail.unwrap_or_default().contains("script"));
    }
}
