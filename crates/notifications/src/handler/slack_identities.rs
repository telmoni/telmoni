//! `/internal/v1/notifications/slack/identities` — a person's own Slack
//! accounts, linked from their account settings by Sign in with Slack, which
//! a press of a button on a notice in Slack acts as: the list, the sign-in's
//! start and finish, and the unlink. Each lane is the person's own, whatever
//! organization they stand in, and reads and writes their rows alone.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::Serialize;
use telmoni_shared::db::tenant_session::person_scope;
use telmoni_shared::digest::sha256_hex;
use telmoni_shared::extract::Json;
use telmoni_shared::{AuthError, TelmoniError, UserId};

use super::connectors::{Authorize, CallbackRequest, STATE_TTL_SECS, mint_state, redirect_uri};
use super::resolve;
use crate::AppState;
use crate::connector::{Provider, slack};
use crate::db::{self, Linked, SlackIdentity};

/// The person behind the bearer.
async fn person_of(state: &AppState, headers: &HeaderMap) -> Result<UserId, TelmoniError> {
    Ok(resolve(state, headers).await?.user_id)
}

/// The Slack app, or a 400: the console offers no link without one.
fn slack_app(state: &AppState) -> Result<&slack::SlackConnector, TelmoniError> {
    state.connectors.slack.as_deref().ok_or_else(|| {
        TelmoniError::BadRequest("Slack is not configured on this deployment".into())
    })
}

/// What the person's account settings show: whether Slack can be linked
/// here, and the accounts linked.
#[derive(Debug, Serialize)]
pub struct IdentityList {
    pub offered: bool,
    pub identities: Vec<SlackIdentity>,
}

/// `GET /internal/v1/notifications/slack/identities` — the person's linked
/// Slack accounts, the oldest first.
pub async fn list_identities(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<IdentityList>, TelmoniError> {
    let person = person_of(&state, &headers).await?;
    let mut tx = person_scope(&state.db, &person).await?;
    let identities = db::slack_identities_of(&mut tx, &person).await?;
    tx.commit().await?;
    Ok(Json(IdentityList {
        offered: state.connectors.slack.is_some(),
        identities,
    }))
}

/// `POST /internal/v1/notifications/slack/identities/authorize` — start the
/// person's Sign in with Slack: its consent page, and the `state` its
/// callback brings back, kept hashed as theirs.
pub async fn authorize_identity(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Authorize>, TelmoniError> {
    let person = person_of(&state, &headers).await?;
    let slack = slack_app(&state)?;
    let raw_state = mint_state()?;
    // Built before the state row, so a URL that will not build mints nothing.
    let url = slack.identity_url(&raw_state, &redirect_uri(&state, Provider::Slack))?;
    let mut tx = person_scope(&state.db, &person).await?;
    db::insert_identity_state(
        &mut tx,
        &sha256_hex(raw_state.as_bytes()),
        &person,
        Provider::Slack,
        STATE_TTL_SECS,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(Authorize {
        url,
        state: raw_state,
    }))
}

/// A Slack account as its link answers it.
#[derive(Debug, Serialize)]
pub struct LinkedIdentity {
    pub team_id: String,
    pub team_name: Option<String>,
}

/// `POST /internal/v1/notifications/slack/identities/callback` — finish the
/// person's sign-in: the `state` spent as theirs, Slack's code exchanged for
/// the account, and the account linked to them. One linked to someone else
/// already is refused, a 409, until they unlink it.
pub async fn link_identity(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CallbackRequest>,
) -> Result<(StatusCode, Json<LinkedIdentity>), TelmoniError> {
    let person = person_of(&state, &headers).await?;
    let slack = slack_app(&state)?;
    let mut tx = person_scope(&state.db, &person).await?;
    let spent =
        db::consume_identity_state(&mut tx, &sha256_hex(req.state.as_bytes()), Provider::Slack)
            .await?;
    tx.commit().await?;
    if !spent {
        return Err(TelmoniError::BadRequest(
            "this sign-in has expired or was already used; start again".into(),
        ));
    }
    let account = slack
        .identify(
            &state.egress,
            &req.code,
            &redirect_uri(&state, Provider::Slack),
        )
        .await?;
    let mut tx = person_scope(&state.db, &person).await?;
    let linked = db::link_slack_identity(
        &mut tx,
        &person,
        &account.team_id,
        &account.user_id,
        account.team_name.as_deref(),
    )
    .await?;
    if linked == Linked::Elsewhere {
        tx.rollback().await?;
        return Err(AuthError::Conflict(
            "that Slack account is linked to another account here; unlink it there first".into(),
        )
        .into());
    }
    tx.commit().await?;
    tracing::info!(user_id = %person, "slack: a person linked their Slack account");
    Ok((
        StatusCode::CREATED,
        Json(LinkedIdentity {
            team_id: account.team_id,
            team_name: account.team_name,
        }),
    ))
}

/// `DELETE /internal/v1/notifications/slack/identities/{team_id}` — the
/// person's Slack account in that workspace unlinked: a press from it acts as
/// nobody again. One not linked is a 404.
pub async fn unlink_identity(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(team_id): Path<String>,
) -> Result<StatusCode, TelmoniError> {
    let person = person_of(&state, &headers).await?;
    let not_found = || AuthError::NotFound("no Slack account of yours in that workspace".into());
    let team_id = slack::slack_id(Some(team_id)).ok_or_else(not_found)?;
    let mut tx = person_scope(&state.db, &person).await?;
    let unlinked = db::unlink_slack_identity(&mut tx, &person, &team_id).await?;
    tx.commit().await?;
    if !unlinked {
        return Err(not_found().into());
    }
    tracing::info!(user_id = %person, "slack: a person unlinked their Slack account");
    Ok(StatusCode::NO_CONTENT)
}
