//! How the SDK reaches Telmoni, and what it sends.

use std::fmt;

use opentelemetry_sdk::trace::ShouldSample;
use serde::{Deserialize, Serialize};

use crate::transport::{host_of, is_this_machine};

/// The machine host; the console's, `https://telmoni.com`, serves people.
pub const DEFAULT_ENDPOINT: &str = "https://api.telmoni.com";

/// How the SDK reaches Telmoni, and what it sends.
///
/// Built by hand, by [`Config::new`] or by [`Config::from_env`]; the client
/// normalizes whichever it is given: the endpoint as the CLI reads one,
/// [`DEFAULT_ENDPOINT`] for a blank one, a blank organization or key unset.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where the platform is: [`DEFAULT_ENDPOINT`] unless `TELMONI_ENDPOINT`
    /// says otherwise.
    pub endpoint: String,
    /// The organization (`org_…`, `TELMONI_ORG`).
    pub organization_id: Option<String>,
    /// The API key (`tm_…`, `TELMONI_API_KEY`); the one bearer ingest
    /// takes. Never serialized, and redacted in `Debug`: a configuration
    /// written out or logged would otherwise carry the key.
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    /// Whether content (prompts, completions, tool arguments and results) may
    /// leave the process. It does so only when this is on *and* the project's
    /// content mode, learned from the platform's answers, is `on`.
    pub capture_content: bool,
    /// The share of runs sent, above 0 and up to 1. Decided per run as it
    /// starts or its record is sent, in the SDK's own processor, never through
    /// the trace's sampled flag, so an application's own sampling is
    /// untouched.
    pub sample_rate: f64,
    /// The sampler whose decision a run takes when it starts a trace of its
    /// own, so an application sampling at a tenth keeps a tenth of its own
    /// spans inside runs as outside them; `None` samples. The run itself is
    /// sent whatever the decision.
    #[serde(skip)]
    pub sampler: Option<Box<dyn ShouldSample>>,
    /// The deployment this process runs in (`production`, `staging`;
    /// `TELMONI_ENVIRONMENT`), sent on every span's resource as
    /// `deployment.environment.name`; `None` sends nothing.
    pub environment: Option<String>,
    /// The release this process runs (a version or a commit;
    /// `TELMONI_RELEASE`), sent on every span's resource as
    /// `service.version`; `None` sends nothing.
    pub release: Option<String>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Destructured, so a field added later has to be placed here: shown,
        // or redacted.
        let Self {
            endpoint,
            organization_id,
            api_key,
            capture_content,
            sample_rate,
            sampler,
            environment,
            release,
        } = self;
        f.debug_struct("Config")
            .field("endpoint", endpoint)
            .field("organization_id", organization_id)
            .field("api_key", &api_key.as_ref().map(|_| "<redacted>"))
            .field("capture_content", capture_content)
            .field("sample_rate", sample_rate)
            .field("sampler", sampler)
            .field("environment", environment)
            .field("release", release)
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_owned(),
            organization_id: None,
            api_key: None,
            capture_content: false,
            sample_rate: 1.0,
            sampler: None,
            environment: None,
            release: None,
        }
    }
}

impl Config {
    /// A configuration with this endpoint, organization and key, normalized;
    /// the rest as [`Config::default`] has it.
    #[must_use]
    pub fn new(
        endpoint: impl Into<String>,
        organization_id: Option<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            organization_id,
            api_key,
            ..Self::default()
        }
        .normalized()
    }

    /// Reads `TELMONI_ENDPOINT`, `TELMONI_API_KEY` and `TELMONI_ORG`, and
    /// `TELMONI_ENVIRONMENT` and `TELMONI_RELEASE` for the deployment, a
    /// blank value being unset, as the CLI reads them. No `OTEL_*` variable is
    /// read: the SDK's tracer provider is built part by part.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            environment: env_var("TELMONI_ENVIRONMENT"),
            release: env_var("TELMONI_RELEASE"),
            ..Self::new(
                env_var("TELMONI_ENDPOINT").unwrap_or_default(),
                env_var("TELMONI_ORG"),
                env_var("TELMONI_API_KEY"),
            )
        }
    }

    /// The endpoint read as the CLI reads it, a blank field unset; applied
    /// again by the client, since one built by hand skips [`Config::new`].
    pub(crate) fn normalized(mut self) -> Self {
        self.endpoint = endpoint_of(&self.endpoint);
        self.organization_id = self.organization_id.as_deref().and_then(unset_if_blank);
        self.api_key = self.api_key.as_deref().and_then(unset_if_blank);
        self.environment = self.environment.as_deref().and_then(unset_if_blank);
        self.release = self.release.as_deref().and_then(unset_if_blank);
        self
    }
}

/// A variable's value, `None` when unset, empty or whitespace; one that is no Unicode is
/// read lossily, so it is refused as in the other SDKs rather than taken as unset.
fn env_var(name: &str) -> Option<String> {
    unset_if_blank(&std::env::var_os(name)?.to_string_lossy())
}

/// The endpoint as the CLI reads one: a host without a scheme gets
/// `https://`, or `http://` for this machine.
fn endpoint_of(given: &str) -> String {
    let given = given.trim();
    // Only what follows the `://` is trimmed, so a second reading changes nothing.
    if let Some((head, tail)) = given.split_once("://") {
        return format!("{head}://{}", trim_tail(tail));
    }
    let endpoint = trim_tail(given);
    if endpoint.is_empty() {
        return DEFAULT_ENDPOINT.to_owned();
    }
    // `https:/host` or `https//host` is a scheme mistyped, not a host: the exporter refuses it.
    let first = endpoint.split([':', '/']).next().unwrap_or_default();
    if first.eq_ignore_ascii_case("http") || first.eq_ignore_ascii_case("https") {
        return endpoint.to_owned();
    }
    let local = host_of(&format!("http://{endpoint}")).is_some_and(is_this_machine);
    format!("{}://{endpoint}", if local { "http" } else { "https" })
}

/// Trailing slashes and whitespace off, so the path appends cleanly.
fn trim_tail(text: &str) -> &str {
    text.trim_end_matches(|c: char| c == '/' || c.is_whitespace())
}

fn unset_if_blank(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
