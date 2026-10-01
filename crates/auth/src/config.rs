use telmoni_shared::Redacted;
use telmoni_shared::config::{env_parse, optional, require};

/// What the module reads from the environment, apart from the identity
/// provider and the mail transport, which the binary that runs it supplies
/// ([`crate::boot`]).
#[derive(Clone)]
pub struct Config {
    /// The DSN this module's pool opens as its own role (`AUTH_DATABASE_URL`).
    pub database_url: String,
    pub service_secret: Redacted,
    /// Optional secondary secret accepted during a rotation window.
    pub service_secret_next: Option<Redacted>,
    /// Whether `POST /test/session` is mounted (`ALLOW_TEST_SESSION`): the
    /// door the console's own test door and the e2e suite mint a signed-in
    /// person through, for anyone holding the service secret. ⚠ Refused at
    /// boot in a Kubernetes pod.
    pub allow_test_session: bool,
    /// The web callback the authorization code flow returns to
    /// (`OIDC_REDIRECT_URI`). Must match a redirect URI registered at the
    /// identity provider.
    pub redirect_uri: String,
    /// Public base URL of the web portal; invite links point here.
    pub app_url: String,
    /// RFC 5322 `From:` for outbound mail (`MAIL_FROM`), which the transport
    /// must be allowed to send as.
    pub mail_from: String,
    /// The address a person is told to write to when a mail was not for them
    /// (`SUPPORT_EMAIL`), optional: the deployment's, never the product's.
    /// Unset, the copy says to write to whoever runs the service.
    pub support_email: Option<String>,
    /// Wall-clock budget for the inline deletion tail (`DELETION_TAIL_BUDGET_MS`,
    /// default 8000). The BFF aborts at 10s, so an unbounded tail would report
    /// "unreachable" for a deletion that durably committed. Expiry is not an
    /// error: it answers 202 and the sweep finishes the tail.
    pub deletion_tail_budget_ms: u64,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            database_url: require("AUTH_DATABASE_URL")?,
            service_secret: require("SERVICE_SECRET")?.into(),
            service_secret_next: optional("SERVICE_SECRET_NEXT").map(Into::into),
            allow_test_session: env_parse("ALLOW_TEST_SESSION", false)?,
            redirect_uri: optional("OIDC_REDIRECT_URI")
                .unwrap_or_else(|| "http://localhost:3000/auth/callback".to_owned()),
            app_url: optional("APP_URL").unwrap_or_else(|| "http://localhost:3000".to_owned()),
            mail_from: optional("MAIL_FROM").unwrap_or_else(telmoni_shared::mail::default_from),
            support_email: optional("SUPPORT_EMAIL"),
            deletion_tail_budget_ms: env_parse("DELETION_TAIL_BUDGET_MS", 8_000)?,
        })
    }
}
