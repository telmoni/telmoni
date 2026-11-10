//! Slack: `incoming-webhook` scope, delivery by POSTing the webhook URL,
//! events and button presses signed with the app's signing secret, and a
//! person's own Slack account linked by Sign in with Slack.
//!
//! **`incoming-webhook` and nothing else, deliberately.** Slack's own picker
//! binds the URL to a channel, public or private, with no bot membership.
//! `chat:write` fails `not_in_channel` anywhere the bot was not invited, and
//! its token is per WORKSPACE. The bot token is kept only for `apps.uninstall`.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use telmoni_shared::digest::{HmacKey, SignatureError, hex_decode, in_replay_window};
use telmoni_shared::net_guard::Egress;
use telmoni_shared::{OrInternal, ProjectId, Redacted, TelmoniError};
use uuid::Uuid;

use super::{
    DeliveryError, Event, EventIncident, Grant, Installed, Retirement, TearDownError, Terminal,
    refused, short_body, truncate, undialable,
};

/// Slack's header block limit.
const MAX_HEADER: usize = 150;
/// Slack's section text limit.
const MAX_SECTION: usize = 3_000;

/// The one scope the app asks for. See the module header.
pub const SCOPE: &str = "incoming-webhook";

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

/// What `apps.uninstall` answers when the token can never do it; any other
/// failure, ours included, is tried again.
const UNINSTALL_FINAL: &[&str] = &[
    "invalid_auth",
    "not_authed",
    "account_inactive",
    "token_revoked",
    "token_expired",
    "no_permission",
    "missing_scope",
    "client_id_token_mismatch",
];

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
        return DeliveryError::terminal(
            Terminal::Retire(Retirement::Revoked),
            format!("slack: {word}"),
        );
    }
    if BAD_TARGET.contains(&word) {
        return DeliveryError::terminal(
            Terminal::Retire(Retirement::Errored),
            format!("slack: {word}"),
        );
    }
    if OUR_BUG.contains(&word) {
        return DeliveryError::terminal(Terminal::Spent { listed: true }, format!("slack: {word}"));
    }
    if status.is_client_error() {
        let named = if word.is_empty() {
            "(empty body)"
        } else {
            word
        };
        return DeliveryError::terminal(
            Terminal::Spent { listed: false },
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

/// Render one notification as a Slack incoming-webhook payload: an incident's
/// opening and its reminders carry its buttons.
#[must_use]
pub fn render(event: &Event<'_>) -> Value {
    let mut blocks = vec![
        json!({
            "type": "header",
            "text": { "type": "plain_text", "text": truncate(event.title, MAX_HEADER), "emoji": true }
        }),
        json!({
            "type": "section",
            "text": { "type": "mrkdwn", "text": truncate(&mrkdwn_escape(event.body), MAX_SECTION) }
        }),
        json!({
            "type": "context",
            "elements": [
                { "type": "mrkdwn", "text": format!("`{}`", event.kind) }
            ]
        }),
    ];
    if let Some(incident) = &event.incident {
        blocks.push(buttons(incident));
    }
    json!({
        "text": mrkdwn_escape(event.title),
        "blocks": blocks,
    })
}

/// What a button on an incident's notice does, as its `action_id` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackAction {
    Acknowledge,
    MuteForAnHour,
    /// A link, which Slack tells the app of all the same.
    OpenTrace,
}

telmoni_shared::wire_enum!(SlackAction {
    Acknowledge => "acknowledge",
    MuteForAnHour => "mute_for_an_hour",
    OpenTrace => "open_trace",
});

/// An incident notice's buttons. Each value names the project and the
/// incident a press is scoped to, never what the presser may do.
fn buttons(incident: &EventIncident<'_>) -> Value {
    let value = button_value(incident.project_id, incident.id);
    let mut elements = vec![
        json!({
            "type": "button",
            "action_id": SlackAction::Acknowledge.as_str(),
            "text": { "type": "plain_text", "text": "Acknowledge" },
            "style": "primary",
            "value": value,
        }),
        json!({
            "type": "button",
            "action_id": SlackAction::MuteForAnHour.as_str(),
            "text": { "type": "plain_text", "text": "Mute for an hour" },
            "value": value,
        }),
    ];
    if let Some(url) = incident.trace_url {
        elements.push(json!({
            "type": "button",
            "action_id": SlackAction::OpenTrace.as_str(),
            "text": { "type": "plain_text", "text": "Open the trace" },
            "url": url,
        }));
    }
    json!({ "type": "actions", "elements": elements })
}

/// The project and the incident a button names, as its value spells them.
#[must_use]
pub fn button_value(project_id: &ProjectId, incident_id: Uuid) -> String {
    format!("{project_id} {incident_id}")
}

/// A button's value read back; `None` for one this server never wrote.
#[must_use]
pub fn parse_button_value(value: &str) -> Option<(ProjectId, Uuid)> {
    let (project, incident) = value.split_once(' ')?;
    Some((ProjectId::try_new(project).ok()?, incident.parse().ok()?))
}

/// A Slack id as this server keeps one: letters and digits, at most 64.
#[must_use]
pub fn slack_id(id: Option<String>) -> Option<String> {
    id.filter(|id| {
        !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

/// Tell the person who pressed a button, and them alone, `text`, at the
/// `response_url` Slack handed with the press. It posts to the channel for
/// thirty minutes, so it is a credential: never logged, never kept.
pub async fn reply(http: &Egress, response_url: &Redacted, text: &str) -> Result<(), String> {
    let resp = http
        .post(response_url.expose())
        .map_err(|why| format!("slack's response_url was refused: {why}"))?
        .json(&json!({
            "response_type": "ephemeral",
            "replace_original": false,
            "text": text,
        }))
        .send()
        .await
        .map_err(|e| format!("slack's response_url unreachable: {}", e.without_url()))?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("slack's response_url returned {status}"))
    }
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
    let timestamp = timestamp.trim();
    let t: i64 = timestamp.parse().map_err(|_| SignatureError::Malformed)?;
    in_replay_window(t, now_unix)?;
    let candidate = signature
        .trim()
        .strip_prefix("v0=")
        .and_then(hex_decode)
        .ok_or(SignatureError::Malformed)?;
    let signed = [b"v0:", timestamp.as_bytes(), b":", body].concat();
    if HmacKey::new(signing_secret.as_bytes()).verify(&signed, &candidate) {
        Ok(())
    } else {
        Err(SignatureError::Mismatch)
    }
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

/// A Web API answer that says only `ok`, as `apps.uninstall`'s does.
#[derive(Debug, Deserialize)]
struct OkResponse {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
}

#[async_trait]
impl Grant for SlackConnector {
    fn authorize_url(&self, state: &str, redirect_uri: &str) -> Result<String, TelmoniError> {
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
        .map_err(|e| TelmoniError::Internal(format!("slack's authorize URL would not build: {e}")))
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
            .or_internal("slack oauth exchange unreachable")?;
        if !resp.status().is_success() {
            return Err(TelmoniError::Internal(format!(
                "slack oauth exchange returned {}",
                resp.status()
            )));
        }
        let body: AccessResponse = resp
            .json()
            .await
            .or_internal("slack oauth exchange body unreadable")?;
        if !body.ok {
            let word = body.error.unwrap_or_else(|| "unknown_error".into());
            return Err(TelmoniError::BadRequest(format!(
                "slack refused the code: {word}"
            )));
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

/// POST one notice to a channel's incoming-webhook `url`, answering the
/// success status.
pub async fn deliver(
    http: &Egress,
    url: &Redacted,
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
    let retry_after = telmoni_shared::http::retry_after(resp.headers())
        .map(|wait| i64::try_from(wait.as_secs()).unwrap_or(i64::MAX));
    let body = short_body(resp).await;
    Err(classify(status, &body, retry_after).answered(status))
}

impl SlackConnector {
    /// Uninstall the app from the workspace `token` was granted in. The row is
    /// the customer's decision, and Slack's answer never rolls it back.
    pub async fn tear_down(&self, http: &Egress, token: &Redacted) -> Result<(), TearDownError> {
        let resp = http
            .post(&format!("{}/api/apps.uninstall", self.api_base))
            .map_err(|why| {
                TearDownError::Unanswered(undialable("slack apps.uninstall", why).to_string())
            })?
            .bearer_auth(token.expose())
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.expose()),
            ])
            .send()
            .await
            .map_err(|e| {
                TearDownError::Unanswered(format!(
                    "slack apps.uninstall unreachable: {}",
                    e.without_url()
                ))
            })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(TearDownError::Unanswered(format!(
                "slack apps.uninstall returned {status}"
            )));
        }
        let body: OkResponse = resp.json().await.map_err(|e| {
            TearDownError::Unanswered(format!(
                "slack apps.uninstall body unreadable: {}",
                e.without_url()
            ))
        })?;
        if body.ok {
            return Ok(());
        }
        let word = body.error.unwrap_or_else(|| "unknown_error".into());
        let why = format!("slack apps.uninstall refused: {word}");
        if UNINSTALL_FINAL.contains(&word.as_str()) {
            Err(TearDownError::Refused(why))
        } else {
            Err(TearDownError::Unanswered(why))
        }
    }
}

/// The one scope Sign in with Slack asks for: who the person is on Slack,
/// their workspace and user, never their address or name.
pub const IDENTITY_SCOPE: &str = "openid";

/// A person's own Slack account, as Sign in with Slack names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackAccount {
    pub team_id: String,
    pub user_id: String,
    /// The workspace's name, for the person's settings to show.
    pub team_name: Option<String>,
}

/// `openid.connect.token`'s answer, the slice read.
#[derive(Debug, Deserialize)]
struct IdentityToken {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    access_token: Option<Redacted>,
}

/// `openid.connect.userInfo`'s answer, the slice read.
#[derive(Debug, Deserialize)]
struct UserInfo {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, rename = "https://slack.com/user_id")]
    user_id: Option<String>,
    #[serde(default, rename = "https://slack.com/team_id")]
    team_id: Option<String>,
    #[serde(default, rename = "https://slack.com/team_name")]
    team_name: Option<String>,
}

/// The longest workspace name kept, as its column holds it.
const TEAM_NAME_MAX_CHARS: usize = 200;

impl SlackConnector {
    /// Slack's consent page for Sign in with Slack, asking [`IDENTITY_SCOPE`]
    /// alone, sent back to `redirect_uri`, the install's own.
    pub fn identity_url(&self, state: &str, redirect_uri: &str) -> Result<String, TelmoniError> {
        reqwest::Url::parse_with_params(
            &format!("{}/openid/connect/authorize", self.api_base),
            &[
                ("response_type", "code"),
                ("scope", IDENTITY_SCOPE),
                ("client_id", self.client_id.as_str()),
                ("state", state),
                ("redirect_uri", redirect_uri),
            ],
        )
        .map(String::from)
        .map_err(|e| TelmoniError::Internal(format!("slack's sign-in URL would not build: {e}")))
    }

    /// The Slack account a Sign in with Slack `code` is for: the code spent
    /// for a token, the token for the account, and the token dropped.
    pub async fn identify(
        &self,
        http: &Egress,
        code: &str,
        redirect_uri: &str,
    ) -> Result<SlackAccount, TelmoniError> {
        let resp = http
            .post(&format!("{}/api/openid.connect.token", self.api_base))
            .map_err(|why| undialable("slack sign-in exchange", why))?
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.expose()),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("grant_type", "authorization_code"),
            ])
            .send()
            .await
            .or_internal("slack sign-in exchange unreachable")?;
        if !resp.status().is_success() {
            return Err(TelmoniError::Internal(format!(
                "slack sign-in exchange returned {}",
                resp.status()
            )));
        }
        let token: IdentityToken = resp
            .json()
            .await
            .or_internal("slack sign-in exchange body unreadable")?;
        if !token.ok {
            let word = token.error.unwrap_or_else(|| "unknown_error".into());
            return Err(TelmoniError::BadRequest(format!(
                "slack refused the code: {word}"
            )));
        }
        let token = token.access_token.ok_or_else(|| {
            TelmoniError::Internal("slack sign-in exchange carried no token".into())
        })?;
        let resp = http
            .post(&format!("{}/api/openid.connect.userInfo", self.api_base))
            .map_err(|why| undialable("slack's account read", why))?
            .bearer_auth(token.expose())
            .send()
            .await
            .or_internal("slack's account read unreachable")?;
        if !resp.status().is_success() {
            return Err(TelmoniError::Internal(format!(
                "slack's account read returned {}",
                resp.status()
            )));
        }
        let info: UserInfo = resp
            .json()
            .await
            .or_internal("slack's account read body unreadable")?;
        if !info.ok {
            let word = info.error.unwrap_or_else(|| "unknown_error".into());
            return Err(TelmoniError::Internal(format!(
                "slack refused the account read: {word}"
            )));
        }
        let (Some(team_id), Some(user_id)) = (slack_id(info.team_id), slack_id(info.user_id))
        else {
            return Err(TelmoniError::Internal(
                "slack named no workspace and user for the sign-in".into(),
            ));
        };
        let team_name = info
            .team_name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty() && name.chars().count() <= TEAM_NAME_MAX_CHARS);
        Ok(SlackAccount {
            team_id,
            user_id,
            team_name,
        })
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
            incident: None,
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
            incident: None,
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
            incident: None,
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
            assert_eq!(
                class(404, word),
                Some(Terminal::Retire(Retirement::Revoked)),
                "{word}"
            );
            assert_eq!(
                class(403, word),
                Some(Terminal::Retire(Retirement::Revoked)),
                "{word}"
            );
        }
        for word in BAD_TARGET {
            assert_eq!(
                class(404, word),
                Some(Terminal::Retire(Retirement::Errored)),
                "{word}"
            );
        }
        for word in OUR_BUG {
            assert_eq!(
                class(400, word),
                Some(Terminal::Spent { listed: true }),
                "{word}"
            );
        }
        assert_eq!(
            class(400, "something_new"),
            Some(Terminal::Spent { listed: false })
        );
        assert_eq!(class(404, ""), Some(Terminal::Spent { listed: false }));
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
        let signed = [format!("v0:{ts}:").as_bytes(), body].concat();
        format!("v0={}", HmacKey::new(secret.as_bytes()).hex(&signed))
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
        let url = c
            .authorize_url("st8", "https://app.example/connect/slack/callback")
            .unwrap();
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

    /// An incident's notice carries Acknowledge, Mute for an hour and Open
    /// the trace, each value naming the project and incident a press is
    /// scoped to; one showing no trace has no link to one, and any other
    /// notice no buttons.
    #[test]
    fn an_incidents_notice_carries_its_buttons() {
        let project = ProjectId::try_new("project_1").unwrap();
        let id = Uuid::now_v7();
        let trace = "https://app.example/org_1/project_1/traces/0190";
        let event = |trace_url| Event {
            id: Uuid::nil(),
            kind: telmoni_shared::types::NotificationKind::IncidentOpened,
            title: "Model calls: alert",
            body: "b",
            incident: Some(EventIncident {
                project_id: &project,
                id,
                trace_url,
            }),
        };
        let v = render(&event(Some(trace)));
        assert_eq!(v["blocks"][3]["type"], "actions");
        let buttons = v["blocks"][3]["elements"].as_array().unwrap();
        let actions: Vec<&str> = buttons
            .iter()
            .map(|button| button["action_id"].as_str().unwrap())
            .collect();
        assert_eq!(actions, ["acknowledge", "mute_for_an_hour", "open_trace"]);
        assert_eq!(buttons[0]["style"], "primary");
        assert_eq!(buttons[2]["url"], trace);
        for button in &buttons[..2] {
            let value = button["value"].as_str().unwrap();
            assert_eq!(parse_button_value(value), Some((project.clone(), id)));
        }
        let no_run = render(&event(None));
        assert_eq!(no_run["blocks"][3]["elements"].as_array().unwrap().len(), 2);
        assert_eq!(
            render(&sample())["blocks"].as_array().unwrap().len(),
            3,
            "no incident, no buttons"
        );
    }

    /// Every action a press names is one a button spells.
    #[test]
    fn a_buttons_action_is_read_back_by_its_word() {
        for action in SlackAction::all() {
            assert_eq!(action.as_str().parse::<SlackAction>(), Ok(action));
        }
        assert!("Acknowledge".parse::<SlackAction>().is_err());
    }

    #[test]
    fn a_value_this_server_never_wrote_names_nothing() {
        for value in [
            String::new(),
            "project_1".into(),
            "project_1 not-a-uuid".into(),
            format!(" {}", Uuid::nil()),
        ] {
            assert_eq!(parse_button_value(&value), None, "{value:?}");
        }
    }

    #[test]
    fn a_slack_id_is_letters_and_digits_alone() {
        assert_eq!(slack_id(Some("U0PRESSER".into())), Some("U0PRESSER".into()));
        for bad in ["", "U 1", "U1;--", "Ü1", &"U".repeat(65)] {
            assert_eq!(slack_id(Some(bad.to_owned())), None, "{bad:?}");
        }
        assert_eq!(slack_id(None), None);
    }

    /// Sign in with Slack asks who the person is and nothing else: never an
    /// address or a name.
    #[test]
    fn signing_in_with_slack_asks_for_openid_alone() {
        let c = SlackConnector::new("cid", Redacted::from("sec"), "https://slack.com/");
        let url = c
            .identity_url("st8", "https://app.example/connect/slack/callback")
            .unwrap();
        assert!(
            url.starts_with("https://slack.com/openid/connect/authorize?"),
            "{url}"
        );
        assert!(url.contains("response_type=code"), "{url}");
        assert!(url.contains("scope=openid&"), "{url}");
        assert!(!url.contains("email") && !url.contains("profile"), "{url}");
        assert!(url.contains("state=st8"), "{url}");
        assert!(!url.contains("sec"), "the secret is not a query parameter");
    }
}
