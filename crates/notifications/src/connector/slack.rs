//! Slack: `incoming-webhook` scope, delivery by POSTing the webhook URL,
//! events signed with the app's signing secret.
//!
//! **`incoming-webhook` and nothing else, deliberately.** Slack's own picker
//! binds the URL to a channel, public or private, with no bot membership.
//! `chat:write` fails `not_in_channel` anywhere the bot was not invited, and
//! its token is per WORKSPACE. The bot token is kept only for `apps.uninstall`.

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha256;
use telmoni_shared::net_guard::Egress;
use telmoni_shared::{AuthError, Redacted, TelmoniError};

use super::{
    Connector, DeliveryError, Event, Grant, Installed, Provider, Terminal, parse_retry_after,
    refused, short_body, truncate, undialable,
};

/// Slack's header block limit.
const MAX_HEADER: usize = 150;
/// Slack's section text limit.
const MAX_SECTION: usize = 3_000;

/// The one scope the app asks for. See the module header.
pub const SCOPE: &str = "incoming-webhook";

/// Reject event timestamps outside this window: Slack's recommended replay
/// bound.
pub const SIGNATURE_TOLERANCE_SECS: i64 = 300;

/// The installation is gone or the token is dead: nothing on our side can make
/// the next post succeed.
const RETIRE: &[&str] = &[
    "no_service",
    "no_service_id",
    "no_active_hooks",
    "no_team",
    "team_disabled",
    "invalid_token",
    "action_prohibited",
    "user_not_found",
];

/// The installation stands and the channel does not.
const BAD_TARGET: &[&str] = &[
    "channel_not_found",
    "channel_is_archived",
    "posting_to_general_channel_denied",
];

/// The body we sent, refused. A renderer bug, and the loop says so.
const OUR_BUG: &[&str] = &["invalid_payload", "no_text", "too_many_attachments"];

/// Classify one non-2xx answer from `hooks.slack.com`.
#[must_use]
pub fn classify(
    status: reqwest::StatusCode,
    body: &str,
    retry_after: Option<i64>,
) -> DeliveryError {
    let word = body.trim();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
    {
        return DeliveryError::transient_after(format!("slack returned {status}"), retry_after);
    }
    if status.is_server_error() {
        return DeliveryError::transient_after(format!("slack returned {status}"), retry_after);
    }
    if RETIRE.contains(&word) {
        return DeliveryError::terminal(Terminal::Retire, format!("slack: {word}"));
    }
    if BAD_TARGET.contains(&word) {
        return DeliveryError::terminal(Terminal::BadTarget, format!("slack: {word}"));
    }
    if OUR_BUG.contains(&word) {
        return DeliveryError::terminal(Terminal::OurBug, format!("slack: {word}"));
    }
    if status.is_client_error() {
        let named = if word.is_empty() {
            "(empty body)"
        } else {
            word
        };
        return DeliveryError::terminal(
            Terminal::Unknown,
            format!("slack returned {status}: {named}"),
        );
    }
    DeliveryError::transient(format!("slack returned {status}"))
}

/// Escape the three characters `mrkdwn` interprets, so a title containing
/// `<!channel>` reads as text rather than paging the channel.
fn mrkdwn_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Render one notification as a Slack incoming-webhook payload.
#[must_use]
pub fn render(event: &Event<'_>) -> Value {
    json!({
        "text": mrkdwn_escape(event.title),
        "blocks": [
            {
                "type": "header",
                "text": { "type": "plain_text", "text": truncate(event.title, MAX_HEADER), "emoji": true }
            },
            {
                "type": "section",
                "text": { "type": "mrkdwn", "text": truncate(&mrkdwn_escape(event.body), MAX_SECTION) }
            },
            {
                "type": "context",
                "elements": [
                    { "type": "mrkdwn", "text": format!("`{}`", event.kind) }
                ]
            }
        ]
    })
}

type HmacSha256 = Hmac<Sha256>;

/// Why an event's signature was refused: one 401 on the wire so a probe learns
/// nothing, and named in the log.
#[derive(Debug, PartialEq, Eq)]
pub enum SignatureError {
    /// A header missing or unparseable.
    Malformed,
    /// The timestamp is outside the replay window.
    Stale,
    /// The HMAC does not match.
    Mismatch,
}

/// Verify `X-Slack-Signature` against the raw body: HMAC-SHA256 over
/// `v0:{timestamp}:{body}`, constant-time, with the timestamp bounded.
pub fn verify_signature(
    signing_secret: &str,
    timestamp: &str,
    signature: &str,
    body: &[u8],
    now_unix: i64,
) -> Result<(), SignatureError> {
    let t: i64 = timestamp
        .trim()
        .parse()
        .map_err(|_| SignatureError::Malformed)?;
    let age = now_unix.saturating_sub(t);
    if !(-SIGNATURE_TOLERANCE_SECS..=SIGNATURE_TOLERANCE_SECS).contains(&age) {
        return Err(SignatureError::Stale);
    }
    let candidate = signature
        .trim()
        .strip_prefix("v0=")
        .and_then(|hex_sig| hex::decode(hex_sig).ok())
        .ok_or(SignatureError::Malformed)?;
    let mut mac = HmacSha256::new_from_slice(signing_secret.as_bytes())
        .map_err(|_| SignatureError::Malformed)?;
    mac.update(b"v0:");
    mac.update(timestamp.trim().as_bytes());
    mac.update(b":");
    mac.update(body);
    mac.verify_slice(&candidate)
        .map_err(|_| SignatureError::Mismatch)
}

/// The Slack app, as registered, and the API origin (a mock in tests).
pub struct SlackConnector {
    client_id: String,
    client_secret: Redacted,
    api_base: String,
}

impl std::fmt::Debug for SlackConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackConnector")
            .field("client_id", &self.client_id)
            .field("api_base", &self.api_base)
            .finish_non_exhaustive()
    }
}

impl SlackConnector {
    #[must_use]
    pub fn new(client_id: impl Into<String>, client_secret: Redacted, api_base: &str) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret,
            api_base: api_base.trim_end_matches('/').to_owned(),
        }
    }
}

/// `oauth.v2.access`'s answer, the slice this service reads.
#[derive(Debug, Deserialize)]
struct AccessResponse {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    access_token: Option<Redacted>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    team: Option<Team>,
    #[serde(default)]
    incoming_webhook: Option<IncomingWebhook>,
}

#[derive(Debug, Deserialize)]
struct Team {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IncomingWebhook {
    channel_id: String,
    #[serde(default)]
    channel: Option<String>,
    url: Redacted,
}

/// A Web API answer that says only `ok`, which is how `apps.uninstall`
#[derive(Debug, Deserialize)]
struct OkResponse {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
}

#[async_trait]
impl Grant for SlackConnector {
    fn authorize_url(&self, state: &str, redirect_uri: &str) -> String {
        reqwest::Url::parse_with_params(
            &format!("{}/oauth/v2/authorize", self.api_base),
            &[
                ("client_id", self.client_id.as_str()),
                ("scope", SCOPE),
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
            .post(&format!("{}/api/oauth.v2.access", self.api_base))
            .map_err(|why| undialable("slack oauth exchange", why))?
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.expose()),
                ("code", code),
                ("redirect_uri", redirect_uri),
            ])
            .send()
            .await
            .map_err(|e| TelmoniError::internal("slack oauth exchange unreachable", e))?;
        if !resp.status().is_success() {
            return Err(TelmoniError::Internal(format!(
                "slack oauth exchange returned {}",
                resp.status()
            )));
        }
        let body: AccessResponse = resp
            .json()
            .await
            .map_err(|e| TelmoniError::internal("slack oauth exchange body unreadable", e))?;
        if !body.ok {
            let word = body.error.unwrap_or_else(|| "unknown_error".into());
            return Err(AuthError::BadRequest(format!("slack refused the code: {word}")).into());
        }
        let team = body
            .team
            .ok_or_else(|| TelmoniError::Internal("slack exchange carried no team".into()))?;
        let hook = body.incoming_webhook.ok_or_else(|| {
            TelmoniError::Internal("slack exchange carried no incoming_webhook".into())
        })?;
        Ok(Installed {
            workspace_id: team.id,
            workspace_name: team.name,
            channel_id: hook.channel_id.clone(),
            channel_name: hook.channel.unwrap_or(hook.channel_id),
            url: hook.url,
            token: body.access_token,
            scopes: body.scope.unwrap_or_else(|| SCOPE.to_owned()),
        })
    }
}

#[async_trait]
impl Connector for SlackConnector {
    fn provider(&self) -> Provider {
        Provider::Slack
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
            .json(&render(event))
            .send()
            .await
            .map_err(|e| {
                DeliveryError::transient(format!("slack webhook POST failed: {}", e.without_url()))
            })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(status.as_u16());
        }
        let retry_after = parse_retry_after(resp.headers());
        let body = short_body(resp).await;
        Err(classify(status, &body, retry_after).answered(status))
    }

    async fn tear_down(
        &self,
        http: &Egress,
        _url: &Redacted,
        token: Option<&Redacted>,
    ) -> Result<(), TelmoniError> {
        let Some(token) = token else {
            return Err(TelmoniError::Internal(
                "slack teardown has no bot token to uninstall with".into(),
            ));
        };
        let resp = http
            .post(&format!("{}/api/apps.uninstall", self.api_base))
            .map_err(|why| undialable("slack apps.uninstall", why))?
            .bearer_auth(token.expose())
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.expose()),
            ])
            .send()
            .await
            .map_err(|e| TelmoniError::internal("slack apps.uninstall unreachable", e))?;
        if !resp.status().is_success() {
            return Err(TelmoniError::Internal(format!(
                "slack apps.uninstall returned {}",
                resp.status()
            )));
        }
        let body: OkResponse = resp
            .json()
            .await
            .map_err(|e| TelmoniError::internal("slack apps.uninstall body unreadable", e))?;
        if body.ok {
            Ok(())
        } else {
            Err(TelmoniError::Internal(format!(
                "slack apps.uninstall refused: {}",
                body.error.unwrap_or_else(|| "unknown_error".into())
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    fn sample() -> Event<'static> {
        Event {
            id: uuid::Uuid::nil(),
            kind: telmoni_shared::types::NotificationKind::ConnectorDisconnected,
            title: "Connector disconnected",
            body: "#alerts stopped accepting posts. Reconnect it from the project's Connectors page.",
        }
    }

    #[test]
    fn the_payload_carries_a_top_level_text_preview() {
        let v = render(&sample());
        assert_eq!(v["text"], "Connector disconnected");
    }

    #[test]
    fn the_header_and_body_are_the_events_own_words() {
        let v = render(&sample());
        assert_eq!(v["blocks"][0]["text"]["text"], "Connector disconnected");
        assert_eq!(
            v["blocks"][1]["text"]["text"],
            "#alerts stopped accepting posts. Reconnect it from the project's Connectors page."
        );
        assert_eq!(
            v["blocks"][2]["elements"][0]["text"],
            "`connector_disconnected`"
        );
    }

    #[test]
    fn the_renderer_invents_no_diagnosis() {
        let rendered = render(&sample()).to_string();
        for invented in ["Diagnostic", "recommendation", "AI ", "root cause"] {
            assert!(!rendered.contains(invented), "invented '{invented}'");
        }
    }

    #[test]
    fn an_over_long_title_is_cut_to_slacks_header_limit_rather_than_refused() {
        let long = "h".repeat(MAX_HEADER + 40);
        let event = Event {
            id: uuid::Uuid::nil(),
            kind: telmoni_shared::types::NotificationKind::MemberAdded,
            title: &long,
            body: "b",
        };
        let v = render(&event);
        assert_eq!(
            v["blocks"][0]["text"]["text"].as_str().unwrap().len(),
            MAX_HEADER
        );
    }

    #[test]
    fn customer_text_cannot_page_the_channel_or_forge_a_link() {
        let event = Event {
            id: uuid::Uuid::nil(),
            kind: telmoni_shared::types::NotificationKind::MemberAdded,
            title: "<!here> t",
            body: "<!channel> api & <https://x|api> is down",
        };
        let v = render(&event);
        assert_eq!(v["text"], "&lt;!here&gt; t");
        assert_eq!(
            v["blocks"][1]["text"]["text"],
            "&lt;!channel&gt; api &amp; &lt;https://x|api&gt; is down"
        );
    }

    /// The load-bearing table, pinned row by row.
    #[test]
    fn every_documented_answer_maps_to_its_class() {
        let class = |status: u16, body: &str| match classify(
            StatusCode::from_u16(status).unwrap(),
            body,
            None,
        ) {
            DeliveryError::Terminal { class, .. } => Some(class),
            DeliveryError::Transient { .. } | DeliveryError::Held { .. } => None,
        };
        for word in RETIRE {
            assert_eq!(class(404, word), Some(Terminal::Retire), "{word}");
            assert_eq!(class(403, word), Some(Terminal::Retire), "{word}");
        }
        for word in BAD_TARGET {
            assert_eq!(class(404, word), Some(Terminal::BadTarget), "{word}");
        }
        for word in OUR_BUG {
            assert_eq!(class(400, word), Some(Terminal::OurBug), "{word}");
        }
        assert_eq!(class(400, "something_new"), Some(Terminal::Unknown));
        assert_eq!(class(404, ""), Some(Terminal::Unknown));
        assert_eq!(class(500, "server_error"), None);
        assert_eq!(class(502, ""), None);
        assert_eq!(class(429, "rate_limited"), None);
        assert_eq!(class(408, ""), None);
    }

    /// A 429 carries the vendor's own wait and is never terminal, whatever
    /// string rides on it.
    #[test]
    fn a_throttle_is_transient_and_keeps_the_retry_after() {
        match classify(StatusCode::TOO_MANY_REQUESTS, "no_service", Some(7)) {
            DeliveryError::Transient { retry_after, .. } => assert_eq!(retry_after, Some(7)),
            DeliveryError::Terminal { .. } | DeliveryError::Held { .. } => {
                panic!("a 429 was terminal")
            }
        }
    }

    #[test]
    fn the_unknown_string_is_named_in_the_reason() {
        let err = classify(StatusCode::BAD_REQUEST, "rollup_error", None);
        assert!(err.message().contains("rollup_error"), "{}", err.message());
    }

    fn sign(secret: &str, ts: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(format!("v0:{ts}:").as_bytes());
        mac.update(body);
        format!("v0={}", hex::encode(mac.finalize().into_bytes()))
    }

    /// A vector computed by an independent implementation (Python's `hmac`
    #[test]
    fn an_independently_computed_vector_verifies() {
        let secret = "8f742231b10e8888abcd99yyyzzz85a5";
        let ts = "1531420618";
        let body = b"token=xyzz0WbapA4vBCDEFasx0q6G&team_id=T1DC2JH3J&team_domain=testteamnow&channel_id=G8PSS9T3V&channel_name=foo&user_id=U2CERLKJA&user_name=roadrunner&command=%2Fwebhook-collect&text=&response_url=https%3A%2F%2Fhooks.slack.com%2Fcommands%2FT1DC2JH3J%2F397700885554%2F96rGlfmibIGlgcZRskXaIFfN&trigger_id=398738663015.81479338432.15f8e9c5ac33bd9394b6ff1a6dbe4b90";
        let sig = "v0=d09b81c3185a443a1e8daf6be5aa832cae4135ec37ec4f4af8eac89d9584d587";
        assert_eq!(
            verify_signature(secret, ts, sig, body, 1_531_420_618),
            Ok(())
        );
        assert_eq!(
            verify_signature(secret, ts, sig, b"tampered", 1_531_420_618),
            Err(SignatureError::Mismatch)
        );
    }

    #[test]
    fn a_stale_or_malformed_signature_is_refused_by_name() {
        let body = br#"{"type":"event_callback"}"#;
        let sig = sign("s3cret", "1000", body);
        assert_eq!(verify_signature("s3cret", "1000", &sig, body, 1000), Ok(()));
        assert_eq!(
            verify_signature("s3cret", "1000", &sig, body, 1000 + 301),
            Err(SignatureError::Stale)
        );
        assert_eq!(
            verify_signature("s3cret", "1000", &sig, body, 1000 - 301),
            Err(SignatureError::Stale)
        );
        assert_eq!(
            verify_signature("s3cret", "not-a-time", &sig, body, 1000),
            Err(SignatureError::Malformed)
        );
        assert_eq!(
            verify_signature("s3cret", "1000", "sha256=abc", body, 1000),
            Err(SignatureError::Malformed)
        );
        assert_eq!(
            verify_signature("s3cret", &i64::MIN.to_string(), &sig, body, 1000),
            Err(SignatureError::Stale)
        );
        assert_eq!(
            verify_signature("other", "1000", &sig, body, 1000),
            Err(SignatureError::Mismatch)
        );
    }

    #[test]
    fn the_authorize_url_asks_for_the_one_scope_and_encodes_the_redirect() {
        let c = SlackConnector::new("cid", Redacted::from("sec"), "https://slack.com/");
        let url = c.authorize_url("st8", "https://app.example/connect/slack/callback");
        assert!(
            url.starts_with("https://slack.com/oauth/v2/authorize?"),
            "{url}"
        );
        assert!(url.contains("scope=incoming-webhook"), "{url}");
        assert!(url.contains("state=st8"), "{url}");
        assert!(
            url.contains("redirect_uri=https%3A%2F%2Fapp.example%2Fconnect%2Fslack%2Fcallback"),
            "{url}"
        );
        assert!(!url.contains("sec"), "the secret is not a query parameter");
    }
}
