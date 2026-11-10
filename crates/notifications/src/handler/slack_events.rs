//! `POST /webhooks/v1/slack` — the events Slack sends about our app, and the
//! presses of the buttons on an incident's notice, both signed with the app's
//! signing secret and checked over the raw body before a byte is parsed.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::FromRequest;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{body::Bytes, extract::State, http::HeaderMap};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::seam::{IncidentAct, IncidentActed};
use telmoni_shared::{
    AuditAction, AuthError, OrganizationId, PRODUCT_NAME, ProjectId, Redacted, TelmoniError,
    TelmoniResourceKind, UserId,
};
use uuid::Uuid;

use crate::AppState;
use crate::connector::Provider;
use crate::connector::slack::{self, SlackAction};
use crate::db::{self, NotificationsLane};
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
) -> Result<Response, TelmoniError> {
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

    if is_form(&headers) {
        return Ok(interaction(&state, body).await?.into_response());
    }

    let envelope: Envelope = serde_json::from_slice(&body)
        .map_err(|e| TelmoniError::BadRequest(format!("unparseable slack event: {e}")))?;
    match envelope {
        Envelope::UrlVerification { challenge } => {
            Ok(Json(json!({ "challenge": challenge })).into_response())
        }
        Envelope::EventCallback {
            team_id: Some(workspace),
            event: Event::Revocation { revocation },
        } if !workspace.is_empty() => {
            let revoked = revoke_workspace(&state, &workspace, revocation).await?;
            Ok(Json(json!({ "ok": true, "revoked": revoked })).into_response())
        }
        Envelope::EventCallback { .. } | Envelope::Other => {
            Ok(Json(json!({ "ok": true })).into_response())
        }
    }
}

/// How Slack posts a press; its events come as JSON.
const FORM: &str = "application/x-www-form-urlencoded";

fn is_form(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with(FORM))
}

/// The form a press comes in: its JSON in one field.
#[derive(Deserialize)]
struct PressForm {
    payload: String,
}

/// The slice of an interaction this lane reads; any other type is answered
/// and otherwise ignored. No `Debug`: it names people on Slack.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Interaction {
    BlockActions {
        user: Presser,
        #[serde(default)]
        team: Option<Team>,
        #[serde(default)]
        actions: Vec<Pressed>,
        /// Where the person is answered, for thirty minutes: a credential.
        #[serde(default)]
        response_url: Option<Redacted>,
    },
    #[serde(other)]
    Other,
}

/// Who pressed: their Slack user, and the workspace they belong to.
#[derive(Deserialize)]
struct Presser {
    id: String,
    #[serde(default)]
    team_id: Option<String>,
}

/// The workspace the press was made in.
#[derive(Deserialize)]
struct Team {
    id: String,
}

/// One button pressed.
#[derive(Deserialize)]
struct Pressed {
    action_id: String,
    #[serde(default)]
    value: Option<String>,
}

/// The press's JSON, from the form it came in, read by axum's own form
/// parser once the signature over the raw bytes has been checked.
async fn payload_of(body: Bytes) -> Result<String, TelmoniError> {
    let refused =
        || TelmoniError::BadRequest("a slack interaction is a form with a payload".into());
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .header(header::CONTENT_TYPE, FORM)
        .body(axum::body::Body::from(body))
        .map_err(|_| refused())?;
    let axum::Form(form) = axum::Form::<PressForm>::from_request(request, &())
        .await
        .map_err(|_| refused())?;
    Ok(form.payload)
}

/// A press to act on: who pressed, and what of which incident.
struct Press {
    team_id: String,
    slack_user_id: String,
    project_id: ProjectId,
    incident_id: Uuid,
    act: IncidentAct,
}

/// A press of a button on a notice, answered at once, as Slack asks within
/// three seconds, and acted on in a task of its own: the act may wait on the
/// organization's chain lock and on auth. The person is told how it went at
/// the press's `response_url`, and nobody else.
async fn interaction(state: &Arc<AppState>, body: Bytes) -> Result<StatusCode, TelmoniError> {
    let payload = payload_of(body).await?;
    // serde's message can quote the payload, which names people.
    let interaction: Interaction = serde_json::from_str(&payload)
        .map_err(|_| TelmoniError::BadRequest("unparseable slack interaction".into()))?;
    let Interaction::BlockActions {
        user,
        team,
        actions,
        response_url,
    } = interaction
    else {
        return Ok(StatusCode::OK);
    };
    let Some(pressed) = actions.into_iter().next() else {
        return Ok(StatusCode::OK);
    };
    let act = match pressed.action_id.parse::<SlackAction>() {
        Ok(SlackAction::Acknowledge) => IncidentAct::Acknowledge,
        Ok(SlackAction::MuteForAnHour) => IncidentAct::MuteForAnHour,
        // A link Slack opened itself, or a button this server never drew.
        Ok(SlackAction::OpenTrace) | Err(_) => return Ok(StatusCode::OK),
    };
    let team_id = slack::slack_id(user.team_id.or(team.map(|team| team.id)));
    let slack_user_id = slack::slack_id(Some(user.id));
    let scoped = pressed.value.as_deref().and_then(slack::parse_button_value);
    let state = state.clone();
    tokio::spawn(async move {
        let answer = match (scoped, team_id, slack_user_id) {
            (None, _, _) => Answer::OutOfDate,
            (Some((project_id, incident_id)), Some(team_id), Some(slack_user_id)) => {
                let press = Press {
                    team_id,
                    slack_user_id,
                    project_id,
                    incident_id,
                    act,
                };
                act_on(&state, &press).await
            }
            (Some(_), _, _) => Answer::Unlinked,
        };
        let Some(response_url) = response_url else {
            return;
        };
        let words = answer.words(&state.config.app_url);
        if let Err(why) = slack::reply(&state.egress, &response_url, &words).await {
            tracing::warn!(error = %why, "slack: the answer to a press could not be posted");
        }
    });
    Ok(StatusCode::OK)
}

/// The press acted on as the person its Slack account is linked to, if any:
/// their role on the incident's project read now, and the act done as the
/// console's own lanes do it.
async fn act_on(state: &AppState, press: &Press) -> Answer {
    let person = match linked_person(state, press).await {
        Ok(Some(person)) => person,
        Ok(None) => return Answer::Unlinked,
        Err(error) => return failed(press, None, &error),
    };
    let Some(telemetry) = state.telemetry.get() else {
        tracing::error!("slack: a press came before telemetry was wired in");
        return Answer::Failed;
    };
    let acting = match state.auth.resolve_person(&person, &press.project_id).await {
        Ok(acting) => acting,
        Err(TelmoniError::BadRequest(_)) => return Answer::Gone,
        Err(error) if refuses(&error) => return Answer::Refused,
        Err(error) => return failed(press, Some(&person), &error),
    };
    match telemetry
        .act_on_incident(&acting, press.incident_id, press.act)
        .await
    {
        Ok(acted) => {
            tracing::info!(
                project_id = %press.project_id,
                incident_id = %press.incident_id,
                user_id = %person,
                act = ?press.act,
                "slack: a press acted on an incident"
            );
            Answer::Acted(acted)
        }
        Err(TelmoniError::Auth(AuthError::NotFound(_))) => Answer::Gone,
        Err(error) if refuses(&error) => Answer::Refused,
        Err(error) => failed(press, Some(&person), &error),
    }
}

/// The person a Slack account is linked to.
async fn linked_person(state: &AppState, press: &Press) -> Result<Option<UserId>, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let person = db::slack_person(&mut tx, &press.team_id, &press.slack_user_id).await?;
    tx.commit().await?;
    Ok(person)
}

/// Whether `error` refuses the person, rather than failing the press: no
/// role that admits the act, or an account on its way out.
fn refuses(error: &TelmoniError) -> bool {
    matches!(
        error,
        TelmoniError::Authz(_) | TelmoniError::Auth(AuthError::Unauthenticated)
    )
}

/// A press that could not be done now, logged by its ids.
fn failed(press: &Press, person: Option<&UserId>, error: &TelmoniError) -> Answer {
    tracing::warn!(
        project_id = %press.project_id,
        incident_id = %press.incident_id,
        user_id = person.map(UserId::as_str),
        error = %error,
        "slack: a press could not be acted on"
    );
    Answer::Failed
}

/// What a press is answered, to the person who pressed alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Acted(IncidentActed),
    /// No account here is linked to the Slack account that pressed.
    Unlinked,
    /// The linked person may not do it on the incident's project.
    Refused,
    /// The incident, or its project, is gone.
    Gone,
    /// The button names nothing this server reads.
    OutOfDate,
    /// It could not be done now.
    Failed,
}

impl Answer {
    /// The answer in Slack's words, `app_url` the console's origin.
    fn words(self, app_url: &str) -> String {
        match self {
            Self::Acted(IncidentActed::Acknowledged) => "Acknowledged. Its reminders stop.".into(),
            Self::Acted(IncidentActed::AcknowledgedAlready) => {
                "This incident is acknowledged already.".into()
            }
            Self::Acted(IncidentActed::ResolvedAlready) => {
                "This incident is resolved already.".into()
            }
            Self::Acted(IncidentActed::Muted { until }) => format!(
                "Muted for an hour: this alert tells no Slack channel or webhook until {}.",
                when(until)
            ),
            Self::Acted(IncidentActed::MutedAlready { until }) => {
                format!("This alert is muted already, until {}.", when(until))
            }
            Self::Unlinked => format!(
                "Link your Slack account to {PRODUCT_NAME} first, from \
                 <{app_url}/settings/account#slack|your account settings>, then press again."
            ),
            Self::Refused => {
                format!("Your {PRODUCT_NAME} account can't do that on this incident's project.")
            }
            Self::Gone => "This incident no longer exists.".into(),
            Self::OutOfDate => {
                format!("This button is out of date. Open the incident in {PRODUCT_NAME} instead.")
            }
            Self::Failed => format!(
                "{PRODUCT_NAME} couldn't do that just now. Try again, or open the incident in \
                 {PRODUCT_NAME}."
            ),
        }
    }
}

/// A time as Slack shows it, in each reader's own zone, or UTC where it
/// cannot.
fn when(at: DateTime<Utc>) -> String {
    format!(
        "<!date^{}^{{time}}|{} UTC>",
        at.timestamp(),
        at.format("%H:%M")
    )
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

    /// A press is answered in words: a time each reader sees in their own
    /// zone, and the way to link an account.
    #[test]
    fn a_press_is_answered_in_words() {
        let until = DateTime::parse_from_rfc3339("2026-11-04T15:04:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            Answer::Acted(IncidentActed::Muted { until }).words("https://app.example"),
            format!(
                "Muted for an hour: this alert tells no Slack channel or webhook until \
                 <!date^{}^{{time}}|15:04 UTC>.",
                until.timestamp()
            )
        );
        assert!(
            Answer::Unlinked
                .words("https://app.example")
                .contains("<https://app.example/settings/account#slack|your account settings>")
        );
        assert!(
            Answer::Failed
                .words("https://app.example")
                .contains(PRODUCT_NAME)
        );
    }

    /// A press's form is told from an event's JSON by its content type alone.
    #[test]
    fn a_press_is_told_from_an_event_by_its_form() {
        let typed = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, value.parse().unwrap());
            is_form(&headers)
        };
        assert!(typed("application/x-www-form-urlencoded"));
        assert!(typed("Application/X-WWW-Form-Urlencoded; charset=utf-8"));
        assert!(!typed("application/json"));
        assert!(!is_form(&HeaderMap::new()));
    }
}
