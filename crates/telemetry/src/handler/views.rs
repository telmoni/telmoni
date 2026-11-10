//! A project's saved dashboards, under [`Resource::View`]: the dashboards
//! page's charts under a name. `GET /internal/v1/telemetry/views`, which
//! every role on the project reads, and `POST`, `PUT /views/{id}` and
//! `DELETE /views/{id}`, which its owners and admins send. A dashboard's
//! charts are checked as its page would draw them, so a saved one always
//! opens. No change to one is audited: a dashboard changes what a page draws,
//! never what anyone may see or what a project keeps.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam::Auth;
use telmoni_shared::{AuthError, ChartMeasure, MetricsGroup, OrInternal, ProjectId, TelmoniError};

use crate::AppState;
use crate::db::{self, ViewRow, ViewWrite};
use crate::handler::chain;

/// The most dashboards a project keeps: more than a menu shows at a glance.
pub const VIEWS_MAX: i64 = 100;
/// The longest a dashboard's name is, in characters, as the table's CHECK
/// has it.
pub const VIEW_NAME_MAX: usize = 60;
/// The unique index a second dashboard of one name, in any case, runs into.
const NAME_TAKEN: &str = "trace_views_project_name_idx";

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

/// The most charts one dashboard draws, each a read of its own.
pub const DASHBOARD_CHARTS_MAX: usize = 12;

/// One of a dashboard's charts: what it draws and, for spend, what splits it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardChart {
    pub measure: ChartMeasure,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<MetricsGroup>,
    /// The tag key a split by `tag` reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl DashboardChart {
    fn checked(self) -> Result<Self, TelmoniError> {
        if self.group.is_some() && self.measure != ChartMeasure::Spend {
            return Err(TelmoniError::BadRequest(
                "a chart is split by a group only where it draws spend".into(),
            ));
        }
        // A tool call and a span's status carry no spend of their own.
        if matches!(
            self.group,
            Some(MetricsGroup::Tool | MetricsGroup::Status | MetricsGroup::Fault)
        ) {
            return Err(TelmoniError::BadRequest(
                "spend is split by model, agent, provider, environment, release, tag, user or \
                 session"
                    .into(),
            ));
        }
        let tag = crate::rollups::tag_of(self.group, self.tag.as_deref())?;
        Ok(Self { tag, ..self })
    }
}

/// A dashboard: what a saved dashboard's `query` holds. Its charts name no
/// user's or session's value, only what splits them, so an end user's
/// erasure leaves none behind here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DashboardOptions {
    /// In the order the page draws them.
    pub charts: Vec<DashboardChart>,
}

impl DashboardOptions {
    /// The charts checked as the page reads them, each once: 1 to
    /// [`DASHBOARD_CHARTS_MAX`] of them.
    fn checked(self) -> Result<Self, TelmoniError> {
        let charts = once(
            self.charts
                .into_iter()
                .map(DashboardChart::checked)
                .collect::<Result<Vec<_>, _>>()?,
        );
        if charts.is_empty() || charts.len() > DASHBOARD_CHARTS_MAX {
            return Err(TelmoniError::BadRequest(format!(
                "a dashboard draws 1 to {DASHBOARD_CHARTS_MAX} charts"
            )));
        }
        Ok(Self { charts })
    }

    /// A saved dashboard's charts that still read, each other left out, so
    /// one chart a later build no longer draws takes none of the rest with it.
    fn readable(query: &Value) -> Self {
        let charts = query
            .get("charts")
            .and_then(Value::as_array)
            .map(|charts| {
                charts
                    .iter()
                    .filter_map(|chart| {
                        serde_json::from_value::<DashboardChart>(chart.clone())
                            .ok()
                            .and_then(|chart| chart.checked().ok())
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            charts: once(charts),
        }
    }
}

/// What saving a dashboard sends: its name and its charts.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewInput {
    pub name: String,
    #[serde(default)]
    pub query: Value,
}

/// A dashboard as it is written: its name trimmed, its charts checked and as
/// JSON.
#[derive(Debug, PartialEq)]
struct Checked {
    name: String,
    query: Value,
}

/// `value` as JSON, which these shapes always are.
fn written<T: Serialize>(value: &T) -> Result<Value, TelmoniError> {
    serde_json::to_value(value).or_internal("a dashboard's charts could not be written")
}

impl ViewInput {
    fn checked(self) -> Result<Checked, TelmoniError> {
        let name = self.name.trim();
        if name.is_empty()
            || name.chars().count() > VIEW_NAME_MAX
            || name.chars().any(char::is_control)
        {
            return Err(TelmoniError::BadRequest(format!(
                "a dashboard's name is 1 to {VIEW_NAME_MAX} characters, none of them a control \
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
        let query = written(
            &serde_json::from_value::<DashboardOptions>(given)
                .map_err(|_| {
                    TelmoniError::BadRequest(
                        "a dashboard's charts are not ones the page draws".into(),
                    )
                })?
                .checked()?,
        )?;
        Ok(Checked {
            name: name.to_owned(),
            query,
        })
    }
}

impl Checked {
    fn write(&self) -> ViewWrite<'_> {
        ViewWrite {
            name: &self.name,
            query: &self.query,
        }
    }
}

/// One saved dashboard, as the lanes answer it.
#[derive(Debug, Serialize)]
pub struct View {
    pub id: Uuid,
    pub name: String,
    pub query: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ViewRow> for View {
    /// A chart a later build no longer draws is left out.
    fn from(row: ViewRow) -> Self {
        let query = written(&DashboardOptions::readable(&row.query)).unwrap_or_else(|_| {
            tracing::warn!(view_id = %row.id, "telemetry: a saved dashboard's charts no longer read");
            json!({ "charts": [] })
        });
        Self {
            id: row.id,
            name: row.name,
            query,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// What the list of dashboards answers.
#[derive(Debug, Serialize)]
pub struct ViewList {
    /// The project's dashboards, by name.
    pub views: Vec<View>,
}

/// A dashboard's id from its path: a UUID, or one the project does not have.
fn view_id(given: &str) -> Result<Uuid, TelmoniError> {
    given
        .parse()
        .map_err(|_| AuthError::NotFound("no dashboard with that id in this project".into()).into())
}

/// A write the unique index refused: a dashboard of that name is the
/// project's already. Any other failure is the database's.
fn name_taken(error: sqlx::Error) -> TelmoniError {
    let taken = error
        .as_database_error()
        .and_then(|db| db.constraint())
        .is_some_and(|constraint| constraint == NAME_TAKEN);
    if taken {
        return AuthError::Conflict("this project has a dashboard by that name already".into())
            .into();
    }
    error.into()
}

/// `GET /internal/v1/telemetry/views` — the project's dashboards, by name.
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

/// `POST /internal/v1/telemetry/views` — a new dashboard of the project.
/// Written under the organization's chain lock, as a content mode is: the
/// project's settings row is made with it where the project has none — the
/// row the organization's purge finds the project's dashboards by — and must
/// be filed under the organization that holds the project now. That row's
/// lock, held to the commit, also puts two saves on one project one after the
/// other, so neither counts past [`VIEWS_MAX`].
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
    let row = chain::write(&state.db, &acting, &project_id, "dashboard", async |tx| {
        save(&*state.auth, &acting, &project_id, tx, &view).await
    })
    .await?;
    tracing::info!(project_id = %project_id, view_id = %row.id, "telemetry: a dashboard saved");
    Ok((StatusCode::CREATED, Json(View::from(row))))
}

/// The save itself, inside the transaction [`create_view`] opened: under the
/// organization's chain lock, the role checked again on auth's second answer,
/// the project's settings row made or re-filed, and the dashboard written
/// where the project keeps fewer than [`VIEWS_MAX`].
async fn save(
    auth: &dyn Auth,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    view: &Checked,
) -> Result<ViewRow, TelmoniError> {
    chain::held(auth, acting, tx, Verb::Create, Resource::View).await?;
    db::ensure_settings(tx, project_id, &acting.organization_id).await?;
    if db::view_count(tx, project_id).await? >= VIEWS_MAX {
        return Err(AuthError::Conflict(format!(
            "a project keeps at most {VIEWS_MAX} dashboards; delete one to save another"
        ))
        .into());
    }
    db::insert_view(tx, project_id, Uuid::new_v4(), view.write())
        .await
        .map_err(name_taken)
}

/// `PUT /internal/v1/telemetry/views/{id}` — one of the project's dashboards,
/// saved over: its name and its charts. Its project's settings row is there
/// already, made when it was first saved.
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
        return Err(AuthError::NotFound("no dashboard with that id in this project".into()).into());
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, view_id = %id, "telemetry: a dashboard saved over");
    Ok(Json(View::from(row)))
}

/// `DELETE /internal/v1/telemetry/views/{id}` — one of the project's
/// dashboards, gone.
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
        return Err(AuthError::NotFound("no dashboard with that id in this project".into()).into());
    }
    tx.commit().await?;
    tracing::info!(project_id = %project_id, view_id = %id, "telemetry: a dashboard deleted");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str, query: Value) -> ViewInput {
        serde_json::from_value(json!({ "name": name, "query": query })).expect("a dashboard's body")
    }

    fn dashboard(query: Value) -> ViewInput {
        named("Spend", query)
    }

    /// A dashboard is kept as the page draws it: its name trimmed, each chart
    /// once and its tag key trimmed.
    #[test]
    fn a_dashboard_is_kept_as_the_page_would_draw_it() {
        let kept = named(
            "  Spend by team ",
            json!({ "charts": [
                { "measure": "spend" },
                { "measure": "spend", "group": "tag", "tag": " team " },
                { "measure": "spend", "group": "tag", "tag": "team" },
                { "measure": "first_token" },
            ] }),
        )
        .checked()
        .expect("kept");
        assert_eq!(kept.name, "Spend by team");
        assert_eq!(
            kept.query,
            json!({ "charts": [
                { "measure": "spend" },
                { "measure": "spend", "group": "tag", "tag": "team" },
                { "measure": "first_token" },
            ] })
        );
    }

    /// ⚠ A dashboard the page would refuse is a 400 that echoes none of it:
    /// a name out of bounds, no chart, too many, a measure or a group there is
    /// none of, a split of anything but spend, a tag split without its key or
    /// a key on another, a user's value, or a list's filter.
    #[test]
    fn a_dashboard_the_page_would_refuse_is_refused() {
        let spend = json!({ "charts": [{ "measure": "spend" }] });
        let many: Vec<Value> = (0..=DASHBOARD_CHARTS_MAX)
            .map(|i| json!({ "measure": "spend", "group": "tag", "tag": format!("k{i}") }))
            .collect();
        for (case, input) in [
            ("Empty", named("", spend.clone())),
            ("Blank", named("   ", spend.clone())),
            ("Long", named(&"d".repeat(VIEW_NAME_MAX + 1), spend.clone())),
            ("Control", named("two\nlines<script>", spend)),
            ("None", dashboard(json!({ "charts": [] }))),
            ("Many", dashboard(json!({ "charts": many }))),
            (
                "Measure",
                dashboard(json!({ "charts": [{ "measure": "cost<script>" }] })),
            ),
            (
                "Group",
                dashboard(json!({ "charts": [{ "measure": "spend", "group": "name<script>" }] })),
            ),
            (
                "Split",
                dashboard(json!({ "charts": [{ "measure": "latency", "group": "model" }] })),
            ),
            (
                "Tool",
                dashboard(json!({ "charts": [{ "measure": "spend", "group": "tool" }] })),
            ),
            (
                "Keyless",
                dashboard(json!({ "charts": [{ "measure": "spend", "group": "tag" }] })),
            ),
            (
                "Stray",
                dashboard(json!({ "charts": [{ "measure": "spend", "tag": "team<script>" }] })),
            ),
            (
                "Value",
                dashboard(json!({ "charts": [{ "measure": "spend", "user": "u-1<script>" }] })),
            ),
            (
                "List",
                dashboard(json!({ "agent": ["support-bot<script>"] })),
            ),
        ] {
            let refused = input.checked().expect_err(case);
            let problem = refused.to_problem_details();
            assert_eq!(problem.status, 400, "{case}");
            assert!(
                !problem.detail.unwrap_or_default().contains("script"),
                "{case}"
            );
        }
    }

    /// A saved dashboard's chart that no longer reads is left out, and the
    /// rest kept.
    #[test]
    fn a_dashboards_chart_that_no_longer_reads_is_left_out() {
        let read = DashboardOptions::readable(&json!({ "charts": [
            { "measure": "spend", "group": "user" },
            { "measure": "cost" },
            { "measure": "tokens", "group": "model" },
            { "measure": "spend", "group": "status" },
            { "measure": "traces" },
        ] }));
        assert_eq!(
            read.charts,
            [
                DashboardChart {
                    measure: ChartMeasure::Spend,
                    group: Some(MetricsGroup::User),
                    tag: None
                },
                DashboardChart {
                    measure: ChartMeasure::Traces,
                    group: None,
                    tag: None
                },
            ]
        );
        assert!(
            DashboardOptions::readable(&json!({ "depth": "steps" }))
                .charts
                .is_empty()
        );
    }
}
