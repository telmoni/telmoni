//! `POST /webhooks/v1/slack` — the events Slack sends about our app.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{body::Bytes, extract::State, http::HeaderMap, response::IntoResponse};
use serde::Deserialize;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Revocation {
    AppUninstalled,
    TokensRevoked,
}

telmoni_shared::wire_enum!(Revocation {
    AppUninstalled => "app_uninstalled",
    TokensRevoked => "tokens_revoked",
});

/// The slice of Slack's envelope this lane reads; any other type is
/// acknowledged and otherwise ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Envelope {
    /// The handshake that registers the URL: the challenge, answered back.
    UrlVerification { challenge: String },
    EventCallback {
        team_id: Option<String>,
        event: Event,
    },
    #[serde(other)]
    Other,
}

/// An event's `type`: a revocation, or anything else Slack sends.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Event {
    Revocation {
        #[serde(rename = "type")]
        revocation: Revocation,
    },
    Other {},
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

    let envelope: Envelope = serde_json::from_slice(&body)
        .map_err(|e| TelmoniError::BadRequest(format!("unparseable slack event: {e}")))?;
    match envelope {
        Envelope::UrlVerification { challenge } => Ok(Json(json!({ "challenge": challenge }))),
        Envelope::EventCallback {
            team_id: Some(workspace),
            event: Event::Revocation { revocation },
        } if !workspace.is_empty() => {
            let revoked = revoke_workspace(&state, &workspace, revocation).await?;
            Ok(Json(json!({ "ok": true, "revoked": revoked })))
        }
        Envelope::EventCallback { .. } | Envelope::Other => Ok(Json(json!({ "ok": true }))),
    }
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
        super::lock_audit_chain(&mut tx, organization).await?;
    }
    // Only these organizations': one the read did not find has no lock held here.
    let retired =
        crate::db::revoke_workspace(&mut tx, Provider::Slack, workspace, &organizations, &reason)
            .await?;
    let ids: Vec<_> = retired.iter().map(|c| c.id).collect();
    crate::db::fail_pending_for_connections(&mut tx, &ids, &reason).await?;
    let mut notices = Vec::new();
    let mut per_organization: BTreeMap<&OrganizationId, Vec<String>> = BTreeMap::new();
    for connection in &retired {
        notices.extend(notify::connector_disconnected(&mut tx, connection, &reason).await?);
        per_organization
            .entry(&connection.organization_id)
            .or_default()
            .push(connection.id.to_string());
    }
    for (organization, ids) in &per_organization {
        emit_audit(
            &mut tx,
            AuditEvent::new(
                organization,
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

    /// The revocation an `event_callback` carries, if any.
    fn revocation_in(event_type: &str) -> Option<Revocation> {
        let body =
            json!({ "type": "event_callback", "team_id": "T1", "event": { "type": event_type } });
        match serde_json::from_value::<Envelope>(body).unwrap() {
            Envelope::EventCallback {
                event: Event::Revocation { revocation },
                ..
            } => Some(revocation),
            _ => None,
        }
    }

    /// Slack's own spelling, which `slack: app_uninstalled` on the row and
    /// the audit row's `event` repeat.
    #[test]
    fn a_revocation_is_read_by_slacks_event_name() {
        assert_eq!(
            revocation_in("app_uninstalled"),
            Some(Revocation::AppUninstalled)
        );
        assert_eq!(
            revocation_in("tokens_revoked"),
            Some(Revocation::TokensRevoked)
        );
        for revocation in Revocation::all() {
            assert_eq!(revocation_in(revocation.as_str()), Some(revocation));
        }
    }

    #[test]
    fn every_other_event_revokes_nothing() {
        for event in ["app_home_opened", "App_Uninstalled", ""] {
            assert_eq!(revocation_in(event), None, "parsed: {event:?}");
        }
        let other: Envelope =
            serde_json::from_value(json!({ "type": "app_rate_limited" })).unwrap();
        assert!(matches!(other, Envelope::Other));
    }

    /// A handshake with no challenge is refused, never answered with an empty one.
    #[test]
    fn a_url_verification_without_a_challenge_is_refused() {
        assert!(serde_json::from_value::<Envelope>(json!({ "type": "url_verification" })).is_err());
    }
}
