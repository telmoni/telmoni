//! How the SDK reaches Telmoni, and what it sends.

use std::fmt;

use opentelemetry_sdk::trace::ShouldSample;
use serde::{Deserialize, Serialize};

/// The machine host; the console's, `https://telmoni.com`, serves people.
pub const DEFAULT_ENDPOINT: &str = "https://api.telmoni.com";

/// How the SDK reaches Telmoni, and what it sends.
///
/// Built by hand, by [`Config::new`] or by [`Config::from_env`]; the client
/// normalizes whichever it is given, so a blank endpoint falls back to
/// [`DEFAULT_ENDPOINT`], trailing slashes go, and a blank organization or key
/// reads as unset.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where the platform is: [`DEFAULT_ENDPOINT`] unless `TELMONI_ENDPOINT`
    /// says otherwise.
    pub endpoint: String,
    /// The organization (`org_…`, `TELMONI_ORG`).
    pub organization_id: Option<String>,
    /// The API key (`telmoni_…`, `TELMONI_API_KEY`); the one bearer ingest
    /// takes. Never serialized, and redacted in `Debug`: a configuration
    /// written out or logged would otherwise carry the key.
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    /// Whether content (prompts, completions, tool arguments and results) may
    /// leave the process. It does so only when this is on *and* the project's
    /// content mode, learned from the platform's answers, is `on`.
    pub capture_content: bool,
    /// The share of runs sent, above 0 and up to 1. Decided per run as it
    /// starts, in the SDK's own processor, never through the trace's sampled
    /// flag, so an application's own sampling is untouched.
    pub sample_rate: f64,
    /// The sampler whose decision a run takes when it starts a trace of its
    /// own, so an application sampling at a tenth keeps a tenth of its own
    /// spans inside runs as outside them; `None` samples. The run itself is
    /// sent whatever the decision.
    #[serde(skip)]
    pub sampler: Option<Box<dyn ShouldSample>>,
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
        } = self;
        f.debug_struct("Config")
            .field("endpoint", endpoint)
            .field("organization_id", organization_id)
            .field("api_key", &api_key.as_ref().map(|_| "<redacted>"))
            .field("capture_content", capture_content)
            .field("sample_rate", sample_rate)
            .field("sampler", sampler)
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

    /// Reads `TELMONI_ENDPOINT`, `TELMONI_API_KEY` and `TELMONI_ORG`, a blank
    /// value being unset, as the CLI reads them. No `OTEL_*` variable is read:
    /// the SDK's tracer provider is built part by part.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(
            env_var("TELMONI_ENDPOINT").unwrap_or_default(),
            env_var("TELMONI_ORG"),
            env_var("TELMONI_API_KEY"),
        )
    }

    /// The configuration as the client uses it: a blank endpoint is the
    /// default, trailing slashes are gone, a blank organization or key is
    /// unset. Applied again by the client, since a configuration built by hand
    /// skips [`Config::new`].
    pub(crate) fn normalized(mut self) -> Self {
        self.endpoint = unset_if_blank(&self.endpoint).map_or_else(
            || DEFAULT_ENDPOINT.to_owned(),
            |endpoint| endpoint.trim_end_matches('/').to_owned(),
        );
        self.organization_id = self.organization_id.as_deref().and_then(unset_if_blank);
        self.api_key = self.api_key.as_deref().and_then(unset_if_blank);
        self
    }
}

/// A variable's value, `None` when it is unset, empty or whitespace.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().as_deref().and_then(unset_if_blank)
}

fn unset_if_blank(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
