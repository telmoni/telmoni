//! The two connectors, and Slack's OAuth grant behind a trait. Slack hands
//! back a webhook URL bound to a channel the person picked; the signed webhook
//! is an endpoint they typed and a secret we minted. From here both are a
//! target to seal, a body to render and an error vocabulary to classify.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use telmoni_shared::net_guard::{Egress, HostRejection};
use telmoni_shared::{
    Redacted, TelmoniError, text::truncate_on_char_boundary, types::NotificationKind,
};
use uuid::Uuid;

pub mod slack;
pub mod webhook;

/// Which kind of connection; the wire spelling is the `provider` column, the
/// path segment and the CHECK.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Slack,
    /// A customer's own HTTPS endpoint, posted signed JSON: the URL is theirs
    /// and the signing secret is ours.
    Webhook,
}

// In the order the page lists them.
telmoni_shared::wire_enum!(Provider {
    Slack => "slack",
    Webhook => "webhook",
});

impl Provider {
    /// The name a customer reads.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Slack => "Slack",
            Self::Webhook => "Webhook",
        }
    }

    /// Whether the vendor's app is installed once per workspace, so that the
    /// last connection into one going means uninstalling it there.
    #[must_use]
    pub const fn installed_per_workspace(self) -> bool {
        match self {
            Self::Slack => true,
            Self::Webhook => false,
        }
    }

    /// Consecutive failed sends that retire a connection whose far end never
    /// answers terminally; `None` leaves it to the classifier.
    #[must_use]
    pub const fn breaker_after(self) -> Option<i32> {
        match self {
            Self::Slack => None,
            Self::Webhook => Some(webhook::BREAKER_AFTER),
        }
    }
}

/// What one install of a vendor app yields.
#[derive(Debug)]
pub struct Installed {
    /// Slack's `team.id`.
    pub workspace_id: String,
    /// Slack's `team.name`, where it sent one.
    pub workspace_name: Option<String>,
    pub channel_id: String,
    /// Slack's channel name.
    pub channel_name: String,
    /// The incoming-webhook URL — the per-connection credential.
    pub url: Redacted,
    /// Slack's bot token, one per workspace, kept for `apps.uninstall` only.
    pub token: Option<Redacted>,
    /// What was granted, as the vendor spelled it.
    pub scopes: String,
}

/// The notification as emitted; no renderer adds facts of its own.
#[derive(Debug, Clone, Copy)]
pub struct Event<'a> {
    /// The delivery row's id — the `Telmoni-Delivery-Id` a receiver dedups on,
    /// stable across every redelivery of one row.
    pub id: Uuid,
    pub kind: NotificationKind,
    /// One-line human summary, as written by the emitting service.
    pub title: &'a str,
    /// The longer human body.
    pub body: &'a str,
}

/// Cut `s` to at most `max` bytes here, rather than have a vendor refuse it
/// with a 400 the loop would treat as terminal.
#[must_use]
pub(crate) fn truncate(s: &str, max: usize) -> String {
    truncate_on_char_boundary(s, max).to_owned()
}

/// Where a connection stands, spelled as the `status` column, its CHECK and
/// the console have it, and as a customer reads it: "this connection is
/// errored". Only an `active` connection is sent to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    /// Sent to: where a connection starts, and where a reconnect or a
    /// rotation puts it back.
    Active,
    /// Its target refuses posts; see [`Retirement::Errored`].
    Errored,
    /// Its installation is dead; see [`Retirement::Revoked`].
    Revoked,
}

telmoni_shared::wire_enum!(ConnectionStatus {
    Active => "active",
    Errored => "errored",
    Revoked => "revoked",
});

/// Why a terminal failure is terminal, and so what it does to the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminal {
    /// The connection goes to the status it names, its queue failed with it,
    /// and the project told.
    Retire(Retirement),
    /// The row fails and the connection stands: the vendor refused the body.
    /// `listed` when the answer is one the table names, which a code change
    /// fixes, so it logs at `error!`; a 4xx whose body names nothing in the
    /// table is treated the same, unlisted.
    Spent { listed: bool },
}

/// A retired connection's status: never `active`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retirement {
    /// The installation is dead; reconnecting is the only cure.
    Revoked,
    /// The target refuses posts — archived, gone, a tripped breaker, a host
    /// we will not dial — while the install stands; a reconnect or a
    /// rotation brings it back.
    Errored,
}

impl Retirement {
    /// The status the connection is left at.
    #[must_use]
    pub const fn status(self) -> ConnectionStatus {
        match self {
            Self::Revoked => ConnectionStatus::Revoked,
            Self::Errored => ConnectionStatus::Errored,
        }
    }
}

/// The taxonomy as a type. `Clone`, because every row on a connection shares
/// the answer to opening it once per batch.
#[derive(Debug, Clone)]
pub enum DeliveryError {
    /// This attempt cannot succeed on a later try; `reason` is the vendor's
    /// answer, bounded. `status` is the HTTP status when the far end answered.
    Terminal {
        class: Terminal,
        reason: String,
        status: Option<u16>,
    },
    /// The far end or the network. Retry with backoff, honouring the vendor's
    /// requested wait as a floor. `status` is `None` when nothing answered.
    Transient {
        message: String,
        retry_after: Option<i64>,
        status: Option<u16>,
    },
    /// Nothing was attempted: the key could not be reached. The row goes back
    /// with its attempt returned, so a KMS outage cannot burn its budget.
    Held { message: String },
}

impl DeliveryError {
    pub(crate) fn terminal(class: Terminal, reason: impl Into<String>) -> Self {
        Self::Terminal {
            class,
            reason: reason.into(),
            status: None,
        }
    }

    pub(crate) fn transient(message: impl Into<String>) -> Self {
        Self::Transient {
            message: message.into(),
            retry_after: None,
            status: None,
        }
    }

    pub(crate) fn transient_after(message: impl Into<String>, retry_after: Option<i64>) -> Self {
        Self::Transient {
            message: message.into(),
            retry_after,
            status: None,
        }
    }

    /// The same failure, recorded as the far end's answer: the delivery log
    /// shows the status a receiver sent back, not only our sentence about it.
    #[must_use]
    pub(crate) fn answered(mut self, code: reqwest::StatusCode) -> Self {
        match &mut self {
            Self::Terminal { status, .. } | Self::Transient { status, .. } => {
                *status = Some(code.as_u16());
            }
            Self::Held { .. } => {}
        }
        self
    }

    /// The HTTP status the far end answered with, if it answered at all.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Terminal { status, .. } | Self::Transient { status, .. } => *status,
            Self::Held { .. } => None,
        }
    }

    /// The sentence `last_error` keeps.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Terminal { reason, .. } => reason,
            Self::Transient { message, .. } | Self::Held { message } => message,
        }
    }
}

/// A status as the delivery log records it: the HTTP client accepts any three
/// digits up to 999, and only 100–599 are statuses; anything else is kept as
/// "an answer we could not read", never written into the log's CHECK.
#[must_use]
pub fn loggable_status(status: Option<u16>) -> Option<u16> {
    status.filter(|code| (100..=599).contains(code))
}

/// A host the guard refused at the send: `errored`, and the page says so.
pub(crate) fn refused(why: HostRejection) -> DeliveryError {
    DeliveryError::terminal(
        Terminal::Retire(Retirement::Errored),
        format!("the target {why}"),
    )
}

/// A host the guard refused on a non-delivery hop: a 500, since the host came
/// from our config or a vendor, never the person.
pub(crate) fn undialable(what: &str, why: HostRejection) -> TelmoniError {
    TelmoniError::Internal(format!("{what}: refusing a host that {why}"))
}

/// What a connection sends with, opened once per batch: Slack's bot token is
/// for uninstalling alone, and a webhook always has its signing secret.
#[derive(Debug)]
pub enum Opened {
    /// Slack's incoming-webhook URL.
    Slack { url: Redacted },
    /// The customer's endpoint, its signing secret, and the rotated one while
    /// its overlap lasts.
    Webhook {
        url: Redacted,
        secret: Redacted,
        prior: Option<Redacted>,
    },
}

impl Opened {
    /// POST one notice over the shared [`Egress`], so the guard and the
    /// no-redirect posture live in one place; answers the success status.
    pub async fn deliver(&self, http: &Egress, event: &Event<'_>) -> Result<u16, DeliveryError> {
        match self {
            Self::Slack { url } => slack::deliver(http, url, event).await,
            Self::Webhook { url, secret, prior } => {
                webhook::deliver(http, url, secret, prior.as_ref(), event).await
            }
        }
    }
}

/// Why an uninstall did not go through.
#[derive(Debug)]
pub enum TearDownError {
    /// Nothing answered, or not yet, here or at the vendor: worth another try.
    Unanswered(String),
    /// The token can never do it: another try would end the same.
    Refused(String),
}

impl std::fmt::Display for TearDownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unanswered(why) | Self::Refused(why) => f.write_str(why),
        }
    }
}

/// The OAuth handshake, Slack's; the signed webhook has none.
#[async_trait]
pub trait Grant: Send + Sync {
    /// Where to send the browser. `redirect_uri` must be sent again, unchanged,
    /// at the exchange.
    fn authorize_url(&self, state: &str, redirect_uri: &str) -> String;

    /// Trade the vendor's `code` for the install.
    async fn exchange(
        &self,
        http: &Egress,
        code: &str,
        redirect_uri: &str,
    ) -> Result<Installed, TelmoniError>;
}

/// The connectors this deployment can connect; one off renders its tile as
/// unavailable. Slack's app is held for its OAuth and `apps.uninstall`; the
/// webhook needs nothing but the key its secrets are sealed with.
#[derive(Clone, Default)]
pub struct Connectors {
    pub slack: Option<std::sync::Arc<slack::SlackConnector>>,
    pub webhook: bool,
}

impl std::fmt::Debug for Connectors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connectors")
            .field("slack", &self.slack.is_some())
            .field("webhook", &self.webhook)
            .finish()
    }
}

impl Connectors {
    /// One connector per registered vendor app, and the webhook when there is
    /// a key.
    #[must_use]
    pub fn from_config(config: &crate::Config) -> Self {
        Self {
            slack: config.slack.as_ref().map(|app| {
                std::sync::Arc::new(slack::SlackConnector::new(
                    app.client_id.clone(),
                    app.client_secret.clone(),
                    &config.slack_api_base,
                ))
            }),
            webhook: config.connector_kek.is_some(),
        }
    }

    /// The OAuth half of a provider; `None` when unavailable, and always for
    /// the webhook.
    #[must_use]
    pub fn grant(&self, provider: Provider) -> Option<std::sync::Arc<dyn Grant>> {
        match provider {
            Provider::Slack => self.slack.clone().map(|c| c as std::sync::Arc<dyn Grant>),
            Provider::Webhook => None,
        }
    }

    /// Whether this deployment can connect `provider`, and send through it.
    #[must_use]
    pub fn enabled(&self, provider: Provider) -> bool {
        match provider {
            Provider::Slack => self.slack.is_some(),
            Provider::Webhook => self.webhook,
        }
    }
}

/// The far end's own instruction for when to come back, from `Retry-After`.
pub(crate) fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<i64> {
    let raw = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .to_owned();
    if let Ok(secs) = raw.parse::<i64>() {
        return Some(secs.max(0));
    }
    let when = chrono::DateTime::parse_from_rfc2822(&raw).ok()?;
    Some((when.timestamp() - chrono::Utc::now().timestamp()).max(0))
}

/// A far end's response body, bounded for a log line, and never a URL: a body
/// echoing the request would echo the credential. Read to a cap in chunks,
/// because a customer's server can send a body that never ends.
pub(crate) async fn short_body(mut resp: reqwest::Response) -> String {
    const MAX: usize = 200;
    const CAP: usize = 4 * 1024;
    let mut buf: Vec<u8> = Vec::new();
    while let Ok(Some(chunk)) = resp.chunk().await {
        buf.extend_from_slice(&chunk);
        if buf.len() >= CAP {
            break;
        }
    }
    // NUL is valid UTF-8 and refused by Postgres `text`: one in a far end's
    // answer failed the write that stores it, and with it the batch's.
    let text = String::from_utf8_lossy(&buf).replace('\0', "");
    truncate(text.trim(), MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        telmoni_shared::test_util::assert_wire_words::<Provider>();
        telmoni_shared::test_util::assert_wire_words::<ConnectionStatus>();
    }

    #[test]
    fn a_provider_parses_its_own_word_alone() {
        assert!("projects".parse::<Provider>().is_err());
        assert!("Slack".parse::<Provider>().is_err());
    }

    #[test]
    fn a_connection_status_parses_its_own_word_alone() {
        for unknown in ["Active", "pending", "disabled", ""] {
            assert!(
                unknown.parse::<ConnectionStatus>().is_err(),
                "parsed: {unknown:?}"
            );
        }
    }

    /// The statuses a retirement promises, neither of them `active`.
    #[test]
    fn a_retirement_names_the_status_it_promises() {
        assert_eq!(Retirement::Revoked.status(), ConnectionStatus::Revoked);
        assert_eq!(Retirement::Errored.status(), ConnectionStatus::Errored);
    }

    /// The webhook is connected by a URL, so there is no handshake to start.
    #[test]
    fn the_webhook_is_a_connector_and_never_a_grant() {
        let connectors = Connectors {
            webhook: true,
            ..Connectors::default()
        };
        assert!(connectors.enabled(Provider::Webhook));
        assert!(connectors.grant(Provider::Webhook).is_none());
        assert!(!connectors.enabled(Provider::Slack));
        assert!(connectors.grant(Provider::Slack).is_none());
    }

    fn headers(value: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    #[test]
    fn retry_after_reads_both_forms_the_rfc_allows() {
        assert_eq!(parse_retry_after(&headers("30")), Some(30));
        assert_eq!(parse_retry_after(&headers("  7 ")), Some(7));
        let soon = (chrono::Utc::now() + chrono::Duration::seconds(120)).to_rfc2822();
        let parsed = parse_retry_after(&headers(&soon)).expect("an http-date parses");
        assert!((100..=120).contains(&parsed), "got {parsed}");
    }

    #[test]
    fn an_unreadable_retry_after_is_no_instruction_rather_than_no_wait() {
        assert_eq!(parse_retry_after(&reqwest::header::HeaderMap::new()), None);
        assert_eq!(parse_retry_after(&headers("later")), None);
        assert_eq!(parse_retry_after(&headers("")), None);
        assert_eq!(parse_retry_after(&headers("-30")), Some(0));
    }

    #[test]
    fn both_variants_surface_their_reason() {
        assert_eq!(DeliveryError::transient("a").message(), "a");
        assert_eq!(
            DeliveryError::terminal(Terminal::Retire(Retirement::Revoked), "b").message(),
            "b"
        );
    }
}
