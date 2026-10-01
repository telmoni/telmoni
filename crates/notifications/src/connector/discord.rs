//! Discord: `webhook.incoming` scope, delivery by POSTing the webhook URL
//! with `wait=true`, teardown by deleting the webhook with its own token. No
//! bot token and no channel names come back, so the tile shows the webhook's
//! name.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use telmoni_shared::net_guard::Egress;
use telmoni_shared::{AuthError, Redacted, TelmoniError};

use super::{
    Connector, DeliveryError, Event, Grant, Installed, Provider, Terminal, parse_retry_after,
    refused, truncate, undialable,
};

/// Discord rejects a whole POST whose embed description exceeds this, which
/// the loop would treat as our bug. Cut it here.
const MAX_DESCRIPTION: usize = 4096;

/// Discord's embed title limit.
const MAX_TITLE: usize = 256;

/// The one scope the application asks for.
pub const SCOPE: &str = "webhook.incoming";

/// Discord's documented codes behind a dead webhook's 404 and 401/403. Never
/// matched on — the status decides — and named so a reason can be looked up.
const INVALID_FORM_BODY: i64 = 50_035;

/// The slice of a Discord error body this service reads.
#[derive(Debug, Default, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    message: Option<String>,
    /// Seconds, decimals allowed, on a 429.
    #[serde(default)]
    retry_after: Option<f64>,
}

/// Classify one non-2xx answer from a Discord webhook.
#[must_use]
pub fn classify(
    status: reqwest::StatusCode,
    body: &str,
    header_retry_after: Option<i64>,
) -> DeliveryError {
    let parsed: ErrorBody = serde_json::from_str(body).unwrap_or_default();
    let code = parsed.code;
    let word = || {
        let m = parsed.message.as_deref().unwrap_or("").trim();
        match (code, m.is_empty()) {
            (Some(c), false) => format!("discord {c}: {m}"),
            (Some(c), true) => format!("discord {c}"),
            (None, false) => format!("discord: {m}"),
            (None, true) => format!("discord returned {status}"),
        }
    };
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let from_body = parsed
            .retry_after
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map(|s| s.ceil().clamp(0.0, 86_400.0))
            .map(|s| {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "clamped to [0, 86400] and rounded up on the line above"
                )]
                let secs = s as i64;
                secs
            });
        return DeliveryError::transient_after(
            format!("discord returned {status}"),
            from_body.or(header_retry_after),
        );
    }
    if status.is_server_error() {
        return DeliveryError::transient_after(
            format!("discord returned {status}"),
            header_retry_after,
        );
    }
    match (status.as_u16(), code) {
        (401 | 403 | 404, _) => DeliveryError::terminal(Terminal::Retire, word()),
        (400, Some(INVALID_FORM_BODY)) => DeliveryError::terminal(Terminal::OurBug, word()),
        (400..=499, _) => DeliveryError::terminal(Terminal::Unknown, word()),
        _ => DeliveryError::transient(word()),
    }
}

/// Render one notification as a Discord webhook payload.
#[must_use]
pub fn render(event: &Event<'_>) -> Value {
    json!({
        "embeds": [
            {
                "title": truncate(event.title, MAX_TITLE),
                "description": truncate(event.body, MAX_DESCRIPTION),
                "footer": { "text": event.kind }
            }
        ]
    })
}

/// The Discord application, as registered.
pub struct DiscordConnector {
    client_id: String,
    client_secret: Redacted,
    api_base: String,
}

impl std::fmt::Debug for DiscordConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordConnector")
            .field("client_id", &self.client_id)
            .field("api_base", &self.api_base)
            .finish_non_exhaustive()
    }
}

impl DiscordConnector {
    #[must_use]
    pub fn new(client_id: impl Into<String>, client_secret: Redacted, api_base: &str) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret,
            api_base: api_base.trim_end_matches('/').to_owned(),
        }
    }
}

/// `POST /oauth2/token`'s answer. The webhook is what the grant IS; the access
/// token beside it grants nothing we use and is not kept.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    scope: Option<String>,
    webhook: Webhook,
}

#[derive(Debug, Deserialize)]
struct Webhook {
    #[serde(default)]
    name: Option<String>,
    channel_id: String,
    guild_id: String,
    url: Redacted,
}

/// A refused exchange: `{ "error": "invalid_grant", "error_description": … }`.
#[derive(Debug, Default, Deserialize)]
struct TokenError {
    #[serde(default)]
    error: Option<String>,
}

#[async_trait]
impl Grant for DiscordConnector {
    fn authorize_url(&self, state: &str, redirect_uri: &str) -> String {
        reqwest::Url::parse_with_params(
            &format!("{}/oauth2/authorize", self.api_base),
            &[
                ("client_id", self.client_id.as_str()),
                ("scope", SCOPE),
                ("response_type", "code"),
                ("state", state),
                ("redirect_uri", redirect_uri),
            ],
        )
        .map(String::from)
        .unwrap_or_default()
    }

    async fn exchange(
        &self,
        http: &Egress,
        code: &str,
        redirect_uri: &str,
    ) -> Result<Installed, TelmoniError> {
        let resp = http
            .post(&format!("{}/api/oauth2/token", self.api_base))
            .map_err(|why| undialable("discord oauth exchange", why))?
            .basic_auth(&self.client_id, Some(self.client_secret.expose()))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
            ])
            .send()
            .await
            .map_err(|e| TelmoniError::internal("discord oauth exchange unreachable", e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::BAD_REQUEST {
            let body: TokenError = resp.json().await.unwrap_or_default();
            let word = body.error.unwrap_or_else(|| "invalid_request".into());
            return Err(AuthError::BadRequest(format!("discord refused the code: {word}")).into());
        }
        if !status.is_success() {
            return Err(TelmoniError::Internal(format!(
                "discord oauth exchange returned {status}"
            )));
        }
        let body: TokenResponse = resp
            .json()
            .await
            .map_err(|e| TelmoniError::internal("discord oauth exchange body unreadable", e))?;
        Ok(Installed {
            workspace_id: body.webhook.guild_id,
            workspace_name: None,
            channel_id: body.webhook.channel_id,
            channel_name: body
                .webhook
                .name
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| "Webhook".into()),
            url: body.webhook.url,
            token: None,
            scopes: body.scope.unwrap_or_else(|| SCOPE.to_owned()),
        })
    }
}

#[async_trait]
impl Connector for DiscordConnector {
    fn provider(&self) -> Provider {
        Provider::Discord
    }

    async fn deliver(
        &self,
        http: &Egress,
        url: &Redacted,
        _token: Option<&Redacted>,
        _prior: Option<&Redacted>,
        event: &Event<'_>,
    ) -> Result<u16, DeliveryError> {
        let resp = http
            .post(url.expose())
            .map_err(refused)?
            .query(&[("wait", "true")])
            .json(&render(event))
            .send()
            .await
            .map_err(|e| {
                DeliveryError::transient(format!(
                    "discord webhook POST failed: {}",
                    e.without_url()
                ))
            })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(status.as_u16());
        }
        let retry_after = parse_retry_after(resp.headers());
        let body = super::short_body(resp).await;
        Err(classify(status, &body, retry_after).answered(status))
    }

    async fn tear_down(
        &self,
        http: &Egress,
        url: &Redacted,
        _token: Option<&Redacted>,
    ) -> Result<(), TelmoniError> {
        let resp = http
            .delete(url.expose())
            .map_err(|why| undialable("discord webhook delete", why))?
            .send()
            .await
            .map_err(|e| TelmoniError::internal("discord webhook delete unreachable", e))?;
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(TelmoniError::Internal(format!(
                "discord webhook delete returned {}",
                resp.status()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;
    use telmoni_shared::types::NotificationKind;

    fn sample() -> Event<'static> {
        Event {
            id: uuid::Uuid::nil(),
            kind: NotificationKind::MemberAdded,
            title: "You were invited to Acme",
            body: "Sam invited you as an admin.",
        }
    }

    #[test]
    fn the_embed_is_the_events_own_words() {
        let v = render(&sample());
        assert_eq!(v["embeds"][0]["title"], "You were invited to Acme");
        assert_eq!(
            v["embeds"][0]["description"],
            "Sam invited you as an admin."
        );
        assert_eq!(v["embeds"][0]["footer"]["text"], "member_added");
    }

    #[test]
    fn an_over_long_body_is_cut_rather_than_rejected_by_discord() {
        let long = "a".repeat(MAX_DESCRIPTION + 500);
        let event = Event {
            id: uuid::Uuid::nil(),
            kind: NotificationKind::MemberAdded,
            title: "t",
            body: &long,
        };
        let v = render(&event);
        assert!(v["embeds"][0]["description"].as_str().unwrap().len() <= MAX_DESCRIPTION);
    }

    #[test]
    fn truncation_never_splits_a_character() {
        let body = "é".repeat(MAX_DESCRIPTION);
        let event = Event {
            id: uuid::Uuid::nil(),
            kind: NotificationKind::MemberAdded,
            title: "t",
            body: &body,
        };
        let v = render(&event);
        let desc = v["embeds"][0]["description"].as_str().unwrap();
        assert!(desc.len() <= MAX_DESCRIPTION);
        assert!(desc.chars().all(|c| c == 'é'));
    }

    fn class(status: u16, body: &str) -> Option<Terminal> {
        match classify(StatusCode::from_u16(status).unwrap(), body, None) {
            DeliveryError::Terminal { class, .. } => Some(class),
            DeliveryError::Transient { .. } | DeliveryError::Held { .. } => None,
        }
    }

    /// The load-bearing table, pinned row by row.
    #[test]
    fn every_documented_answer_maps_to_its_class() {
        assert_eq!(
            class(404, r#"{"code":10015,"message":"Unknown Webhook"}"#),
            Some(Terminal::Retire)
        );
        assert_eq!(
            class(401, r#"{"code":50027,"message":"Invalid Webhook Token"}"#),
            Some(Terminal::Retire)
        );
        assert_eq!(
            class(403, r#"{"code":50027,"message":"Invalid Webhook Token"}"#),
            Some(Terminal::Retire)
        );
        assert_eq!(class(404, "not json"), Some(Terminal::Retire));
        assert_eq!(
            class(401, r#"{"message":"401: Unauthorized","code":0}"#),
            Some(Terminal::Retire)
        );
        assert_eq!(
            class(400, r#"{"code":50035,"message":"Invalid Form Body"}"#),
            Some(Terminal::OurBug)
        );
        assert_eq!(class(400, r#"{"code":99999}"#), Some(Terminal::Unknown));
        assert_eq!(class(429, "not json"), None);
        assert_eq!(class(500, ""), None);
        assert_eq!(class(502, "<html>"), None);
        assert_eq!(class(429, r#"{"retry_after":1.5}"#), None);
    }

    #[test]
    fn a_throttle_paces_on_the_bodys_retry_after_rounded_up() {
        match classify(
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"message":"You are being rate limited.","retry_after":0.4,"global":false}"#,
            Some(30),
        ) {
            DeliveryError::Transient { retry_after, .. } => assert_eq!(retry_after, Some(1)),
            DeliveryError::Terminal { .. } | DeliveryError::Held { .. } => {
                panic!("a 429 was terminal")
            }
        }
        match classify(StatusCode::TOO_MANY_REQUESTS, "{}", Some(30)) {
            DeliveryError::Transient { retry_after, .. } => assert_eq!(retry_after, Some(30)),
            DeliveryError::Terminal { .. } | DeliveryError::Held { .. } => {
                panic!("a 429 was terminal")
            }
        }
    }

    #[test]
    fn the_reason_names_the_code_and_the_message() {
        let err = classify(
            StatusCode::NOT_FOUND,
            r#"{"code":10015,"message":"Unknown Webhook"}"#,
            None,
        );
        assert_eq!(err.message(), "discord 10015: Unknown Webhook");
    }

    #[test]
    fn the_authorize_url_asks_for_a_code_under_the_one_scope() {
        let c = DiscordConnector::new("cid", Redacted::from("sec"), "https://discord.com");
        let url = c.authorize_url("st8", "https://app.example/connect/discord/callback");
        assert!(
            url.starts_with("https://discord.com/oauth2/authorize?"),
            "{url}"
        );
        assert!(url.contains("scope=webhook.incoming"), "{url}");
        assert!(url.contains("response_type=code"), "{url}");
        assert!(url.contains("state=st8"), "{url}");
        assert!(!url.contains("sec"));
    }
}
