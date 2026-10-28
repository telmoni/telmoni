//! The layout a project's sessions and traces open in, under
//! [`Resource::View`]: `GET /internal/v1/telemetry/session-layout`, which every
//! role on the project reads, and `PUT`, which its owners and admins send —
//! one of the project's saved layouts, which stands until it is deleted,
//! when the project opens in the console's own default again, or none, for
//! that default. Not audited, as the views are not: it changes what a page
//! opens on, never what anyone may see.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{AuthError, TelmoniError};

use crate::handler::chain;
use crate::{AppState, db};

/// What both lanes answer: the saved layout the project opens in, or none
/// for the default.
#[derive(Debug, Serialize)]
pub struct SessionLayoutSetting {
    pub view_id: Option<Uuid>,
}

/// The body of a change: a saved layout, or `null` — or nothing — for the
/// default.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLayoutChange {
    #[serde(default)]
    pub view_id: Option<Uuid>,
}

/// `GET /internal/v1/telemetry/session-layout` — what the project's sessions
/// and traces open in.
pub async fn get_session_layout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<SessionLayoutSetting>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::View)?
        .project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    let view_id = db::session_layout(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(Json(SessionLayoutSetting { view_id }))
}

/// `PUT /internal/v1/telemetry/session-layout` — set what they open in. Written
/// under the organization's chain lock, as a view is saved: the project's
/// settings row is made with it where the project has none, filed under the
/// organization that holds the project now.
pub async fn put_session_layout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(change): Json<SessionLayoutChange>,
) -> Result<Json<SessionLayoutSetting>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    // Refused before any lock is taken, so nobody below an admin waits on
    // the organization's chain to be told no.
    let project_id = acting
        .require_project(Verb::Update, Resource::View)?
        .project_id
        .clone();
    let view_id = change.view_id;
    chain::write(
        &state.db,
        &acting,
        &project_id,
        "session layout",
        async |tx| {
            chain::held(&*state.auth, &acting, tx, Verb::Update, Resource::View).await?;
            if let Some(id) = view_id
                && !db::layout_exists(tx, &project_id, id).await?
            {
                return Err(
                    AuthError::NotFound("no layout with that id in this project".into()).into(),
                );
            }
            db::set_session_layout(tx, &project_id, &acting.organization_id, view_id).await?;
            Ok(())
        },
    )
    .await?;
    tracing::info!(
        project_id = %project_id,
        view_id = ?view_id,
        "telemetry: a project's session layout set"
    );
    Ok(Json(SessionLayoutSetting { view_id }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A change names a saved layout, or the default by `null` or by saying
    /// nothing; any other key, or an id of another shape, is refused.
    #[test]
    fn a_change_names_a_layout_or_the_default() {
        let id = Uuid::new_v4();
        let read = |body: serde_json::Value| serde_json::from_value::<SessionLayoutChange>(body);
        assert_eq!(read(serde_json::json!({})).expect("nothing").view_id, None);
        assert_eq!(
            read(serde_json::json!({ "view_id": null }))
                .expect("null")
                .view_id,
            None
        );
        assert_eq!(
            read(serde_json::json!({ "view_id": id }))
                .expect("a layout")
                .view_id,
            Some(id)
        );
        for body in [
            serde_json::json!({ "layout": "simple" }),
            serde_json::json!({ "view_id": "not-a-view" }),
        ] {
            assert!(read(body.clone()).is_err(), "{body}");
        }
    }
}
