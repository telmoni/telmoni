//! `POST /webhooks/v1/slack` — the events Slack sends about our app.

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

/// The events that retire a workspace's connections, by Slack's name for
/// them, which the stored reason and the audit row both carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Revocation {
    AppUninstalled,
    TokensRevoked,
}

telmoni_shared::wire_enum!(Revocation {
    AppUninstalled => "app_uninstalled",
    TokensRevoked => "tokens_revoked",
});

impl Revocation {
    /// The revocation an event's `type` names; `None` for every other event,
    /// which is acknowledged and otherwise ignored.
    fn parse(event_type: &str) -> Option<Self> {
        event_type.parse().ok()
    }
}

/// `POST /webhooks/v1/slack`.
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
        if let Some(revocation) = Revocation::parse(event_type)
            && !workspace.is_empty()
        {
            let revoked = revoke_workspace(&state, workspace, revocation).await?;
            return Ok(Json(json!({ "ok": true, "revoked": revoked })));
        }
    }

    Ok(Json(json!({ "ok": true })))
}

/// Retire the connections into `workspace`, across tenants, on Slack's word,
/// each organization's chain lock taken first, by id.
async fn revoke_workspace(
    state: &Arc<AppState>,
    workspace: &str,
    revocation: Revocation,
) -> Result<usize, TelmoniError> {
    let event_type = revocation.as_str();
    let reason = format!("slack: {event_type}");
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let organizations =
        crate::db::workspace_organizations(&mut tx, Provider::Slack, workspace).await?;
    for organization in &organizations {
        super::lock_audit_chain(
            &mut tx,
            &OrganizationId::from_trusted(organization.as_str()),
        )
        .await?;
    }
    // Only these organizations': one the read did not find has no lock held here.
    let retired =
        crate::db::revoke_workspace(&mut tx, Provider::Slack, workspace, &organizations, &reason)
            .await?;
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
            AuditEvent::new(
                &organization_id,
                Actor::External("slack"),
                AuditAction::Updated,
                TelmoniResourceKind::Connector,
            )
            .details(json!({
                "event": event_type,
                "workspace": workspace,
                "revoked": ids,
            })),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Slack's own spelling, which `slack: app_uninstalled` on the row and
    /// the audit row's `event` repeat.
    #[test]
    fn a_revocation_is_read_by_slacks_event_name() {
        assert_eq!(
            Revocation::parse("app_uninstalled"),
            Some(Revocation::AppUninstalled)
        );
        assert_eq!(
            Revocation::parse("tokens_revoked"),
            Some(Revocation::TokensRevoked)
        );
        for revocation in Revocation::all() {
            assert_eq!(Revocation::parse(revocation.as_str()), Some(revocation));
        }
    }

    #[test]
    fn every_other_event_revokes_nothing() {
        for event in [
            "url_verification",
            "event_callback",
            "app_home_opened",
            "App_Uninstalled",
            "",
        ] {
            assert_eq!(Revocation::parse(event), None, "parsed: {event:?}");
        }
    }
}
