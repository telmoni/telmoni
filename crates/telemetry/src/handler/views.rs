//! A project's saved views of its trace list, under [`Resource::View`]:
//! `GET /internal/telemetry/views`, which every role on the project reads,
//! and `POST`, `PUT /views/{id}` and `DELETE /views/{id}`, which its owners
//! and admins send. A view is the list's query and the columns it showed,
//! under a name; the query is checked as the list would read it, so a view
//! always opens. No change to a view is audited: a view changes what a page
//! opens on, never what anyone may see or what a project keeps.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam::Auth;
use telmoni_shared::{
    AuthError, ProjectId, SortDirection, SpanStatus, TelmoniError, TraceColumn, TraceSort,
};

use crate::AppState;
use crate::db::{self, ViewRow, ViewWrite};
use crate::handler::chain;
use crate::handler::traces::{TraceQuery, parse_limit};

/// The most views one project keeps: more than a menu shows at a glance.
pub const VIEWS_MAX: i64 = 100;
/// The longest a view's name is, in characters, as the table's CHECK has it.
pub const VIEW_NAME_MAX: usize = 60;
/// The unique index a second view of one name, in any case, runs into.
const NAME_TAKEN: &str = "trace_views_project_name_idx";

/// A view's query: the list's, each filter's values a list. A key left out
/// narrows nothing, or asks for the list's default.
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
            sort: self.sort,
            dir: self.dir,
            days: self.days,
            limit: self.limit,
        })
    }
}

/// What saving a view sends: its name, the list's query and the columns
/// shown. The trace's id is always shown, and is none of the columns.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewInput {
    pub name: String,
    #[serde(default)]
    pub query: ViewQuery,
    #[serde(default)]
    pub columns: Vec<TraceColumn>,
}

/// A view as it is written: its name trimmed, its query checked and as JSON,
/// its columns each once, in the list's order.
#[derive(Debug, PartialEq)]
struct Checked {
    name: String,
    query: serde_json::Value,
    columns: Vec<String>,
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
        let query = serde_json::to_value(self.query.checked()?).map_err(|e| {
            TelmoniError::Internal(format!("a view's query could not be written: {e}"))
        })?;
        let columns = TraceColumn::all()
            .into_iter()
            .filter(|column| self.columns.contains(column))
            .map(|column| column.as_str().to_owned())
            .collect();
        Ok(Checked {
            name: name.to_owned(),
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

/// One saved view, as the lanes answer it.
#[derive(Debug, Serialize)]
pub struct View {
    pub id: Uuid,
    pub name: String,
    pub query: ViewQuery,
    pub columns: Vec<TraceColumn>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ViewRow> for View {
    /// A row's query no longer read as a view's — written before a filter was
    /// renamed — is answered empty, so the view opens on the whole list
    /// rather than taking the menu down with it; a column no longer the
    /// list's is left out.
    fn from(row: ViewRow) -> Self {
        let query = serde_json::from_value(row.query).unwrap_or_else(|e| {
            tracing::warn!(view_id = %row.id, error = %e, "telemetry: a saved view's query no longer reads");
            ViewQuery::default()
        });
        Self {
            id: row.id,
            name: row.name,
            query,
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
    /// The project's views, by name.
    pub views: Vec<View>,
}

/// A view's id from its path: a UUID, or a view the project does not have.
fn view_id(given: &str) -> Result<Uuid, TelmoniError> {
    given
        .parse()
        .map_err(|_| AuthError::NotFound("no view with that id in this project".into()).into())
}

/// A write the unique index refused: a view of that name is the project's
/// already. Any other failure is the database's.
fn name_taken(error: sqlx::Error) -> TelmoniError {
    let taken = error
        .as_database_error()
        .and_then(|db| db.constraint())
        .is_some_and(|constraint| constraint == NAME_TAKEN);
    if taken {
        return AuthError::Conflict("this project has a view by that name already".into()).into();
    }
    error.into()
}

/// `GET /internal/telemetry/views` — the project's views, by name.
pub async fn list_views(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ViewList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::View)?
        .project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = db::views(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(Json(ViewList {
        views: rows.into_iter().map(View::from).collect(),
    }))
}

/// `POST /internal/telemetry/views` — a new view of the project. Written
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
    let mut tx = project_scope(&state.db, &project_id)
        .await?
        .bind_organization(&acting.organization_id)
        .await?;
    let row = match save(&*state.auth, &acting, &project_id, &mut tx, &view).await {
        Ok(row) => row,
        Err(refused) => {
            rolled_back(tx.rollback().await, &project_id);
            return Err(refused);
        }
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, view_id = %row.id, "telemetry: a view saved");
    Ok((StatusCode::CREATED, Json(View::from(row))))
}

/// The save itself, inside the transaction [`create_view`] opened: under the
/// organization's chain lock, the role checked again on auth's second answer,
/// the project's settings row made or re-filed, and the view written where
/// the project keeps fewer than [`VIEWS_MAX`].
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
    if db::view_count(tx, project_id).await? >= VIEWS_MAX {
        return Err(AuthError::Conflict(format!(
            "a project keeps at most {VIEWS_MAX} views; delete one to save another"
        ))
        .into());
    }
    db::insert_view(tx, project_id, Uuid::new_v4(), view.write())
        .await
        .map_err(name_taken)
}

/// `PUT /internal/telemetry/views/{id}` — one of the project's views, saved
/// over: its name, its query and its columns. Its project's settings row is
/// there already, made when it was first saved.
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
    let Some(row) = db::update_view(&mut tx, project_id, id, view.write())
        .await
        .map_err(name_taken)?
    else {
        return Err(AuthError::NotFound("no view with that id in this project".into()).into());
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, view_id = %id, "telemetry: a view saved over");
    Ok(Json(View::from(row)))
}

/// `DELETE /internal/telemetry/views/{id}` — one of the project's views,
/// gone.
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

/// A refused save let go of: nothing was written, and the chain lock goes
/// with the transaction rather than whenever the pool next sees the
/// connection.
fn rolled_back(result: sqlx::Result<()>, project_id: &ProjectId) {
    if let Err(e) = result {
        tracing::warn!(
            project_id = %project_id,
            error = %e,
            "telemetry: a refused view could not be rolled back; its connection drops it"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, query: serde_json::Value, columns: &[&str]) -> ViewInput {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "query": query,
            "columns": columns,
        }))
        .expect("a view's body")
    }

    /// A view's name is trimmed, its query kept as the list reads it — each
    /// value trimmed and once — and its columns each once, in the list's
    /// order.
    #[test]
    fn a_view_is_kept_as_the_list_would_read_it() {
        let checked = input(
            "  Errors this week ",
            serde_json::json!({
                "agent": [" support-bot ", "support-bot", ""],
                "status": ["error"],
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
        assert_eq!(
            checked.query,
            serde_json::json!({
                "agent": ["support-bot"],
                "status": ["error"],
                "sort": "tokens",
                "dir": "asc",
                "days": 7,
                "limit": 50,
            })
        );
        assert_eq!(checked.columns, ["started", "status", "duration"]);
        let empty = input("All", serde_json::json!({}), &[])
            .checked()
            .expect("a view of the whole list");
        assert_eq!(empty.query, serde_json::json!({}));
    }

    /// ⚠ A name or a query the list would refuse is a 400 that echoes none
    /// of it, and a query naming a filter there is none of is refused as it
    /// arrives.
    #[test]
    fn a_view_the_list_would_refuse_is_refused() {
        for (name, query) in [
            ("", serde_json::json!({})),
            ("   ", serde_json::json!({})),
            (&*"v".repeat(VIEW_NAME_MAX + 1), serde_json::json!({})),
            ("two\nlines<script>", serde_json::json!({})),
            ("Late", serde_json::json!({ "days": 0 })),
            ("Long", serde_json::json!({ "days": 361 })),
            ("Big", serde_json::json!({ "limit": 501 })),
            ("Agent", serde_json::json!({ "agent": ["a".repeat(201)] })),
        ] {
            let refused = input(name, query, &[]).checked().expect_err(name);
            let problem = refused.to_problem_details();
            assert_eq!(problem.status, 400, "{name}");
            assert!(!problem.detail.unwrap_or_default().contains("script"));
        }
        let unknown = serde_json::from_value::<ViewInput>(serde_json::json!({
            "name": "x",
            "query": { "spend": ["high"] },
        }));
        assert!(unknown.is_err(), "a filter there is none of");
        let column = serde_json::from_value::<ViewInput>(serde_json::json!({
            "name": "x",
            "columns": ["run_id"],
        }));
        assert!(column.is_err(), "a column the list hides none of");
    }

    /// A view whose stored query no longer reads opens on the whole list,
    /// and a column no longer the list's is left out.
    #[test]
    fn a_row_that_no_longer_reads_opens_on_the_whole_list() {
        let row = ViewRow {
            id: Uuid::nil(),
            name: "Old".into(),
            query: serde_json::json!({ "customer": ["c-1"] }),
            columns: vec!["started".into(), "spend".into()],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let view = View::from(row);
        assert_eq!(view.query, ViewQuery::default());
        assert_eq!(view.columns, [TraceColumn::Started]);
    }
}
