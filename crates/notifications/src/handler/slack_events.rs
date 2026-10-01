//! `POST /webhooks/slack` — the events Slack sends about our app.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{body::Bytes, extract::State, http::HeaderMap, response::IntoResponse};
use serde_json::json;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::{AuditAction, AuthError, OrganizationId, TelmoniError, TelmoniResourceKind};

use crate::AppState;
use crate::connector::{Provider, slack};
use crate::db::NotificationsLane;
use crate::delivery;
use crate::notify;

/// The events that retire a workspace's connections.
const REVOKING_EVENTS: &[&str] = &["app_uninstalled", "tokens_revoked"];

/// `POST /webhooks/slack`.
pub async fn slack_events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, TelmoniError> {
    let Some(secret) = &state.config.slack_signing_secret else {
        tracing::error!("slack event received but SLACK_SIGNING_SECRET is not configured");
        return Err(TelmoniError::Internal(
            "slack events are not configured".into(),
        ));
    };
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .ok_or(AuthError::Unauthenticated)
    };
    let timestamp = header("x-slack-request-timestamp")?;
    let signature = header("x-slack-signature")?;
    let now = chrono::Utc::now().timestamp();
    if let Err(why) = slack::verify_signature(secret.expose(), timestamp, signature, &body, now) {
        tracing::warn!(?why, "slack event signature refused");
        return Err(AuthError::Unauthenticated.into());
    }

    let payload: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| AuthError::BadRequest(format!("unparseable slack event: {e}")))?;
    let kind = payload
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or_default();

    if kind == "url_verification" {
        let challenge = payload
            .get("challenge")
            .and_then(|c| c.as_str())
            .unwrap_or_default();
        return Ok(Json(json!({ "challenge": challenge })));
    }

    if kind == "event_callback" {
        let event_type = payload
            .pointer("/event/type")
            .and_then(|t| t.as_str())
            .unwrap_or_default();
        let workspace = payload
            .get("team_id")
            .and_then(|t| t.as_str())
            .unwrap_or_default();
        if REVOKING_EVENTS.contains(&event_type) && !workspace.is_empty() {
            let revoked = revoke_workspace(&state, workspace, event_type).await?;
            return Ok(Json(json!({ "ok": true, "revoked": revoked })));
        }
    }

    Ok(Json(json!({ "ok": true })))
}

/// Retire every connection into `workspace`, across tenants, on Slack's word.
pub(crate) async fn revoke_workspace(
    state: &Arc<AppState>,
    workspace: &str,
    event_type: &str,
) -> Result<usize, TelmoniError> {
    let reason = format!("slack: {event_type}");
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let retired = crate::db::revoke_workspace(&mut tx, Provider::Slack, workspace, &reason).await?;
    let ids: Vec<_> = retired.iter().map(|c| c.id).collect();
    crate::db::fail_pending_for_connections(&mut tx, &ids, &reason).await?;
    let mut notices = Vec::new();
    let mut per_organization: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for connection in &retired {
        notices.extend(notify::connector_disconnected(&mut tx, connection, &reason).await?);
        per_organization
            .entry(connection.organization_id.clone())
            .or_default()
            .push(connection.id.to_string());
    }
    for (organization, ids) in &per_organization {
        let organization_id = OrganizationId::try_new(organization.as_str())
            .map_err(|e| TelmoniError::Internal(format!("stored organization id: {e}")))?;
        emit_audit(
            &mut tx,
            AuditEvent {
                organization_id: &organization_id,
                in_project: None,
                actor: Actor::External("slack"),
                action: AuditAction::Updated,
                resource_kind: TelmoniResourceKind::Connector,
                resource_id: None,
                request_id: None,
                ip_address: None,
                user_agent: None,
                metadata: Some(json!({
                    "event": event_type,
                    "workspace": workspace,
                    "revoked": ids,
                })),
            },
        )
        .await?;
    }
    tx.commit().await?;
    if !retired.is_empty() {
        tracing::error!(
            workspace,
            event = event_type,
            revoked = retired.len(),
            organizations = per_organization.len(),
            "slack workspace uninstalled; its connections are revoked"
        );
    }
    delivery::spawn_first_attempts(state, notices);
    Ok(retired.len())
}
