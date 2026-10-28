//! The agent's configuration, loaded once from the environment.
//!
//! Three levels. `AGENT_DATABASE_URL` unset: the deployment has no agent at
//! all, and its routes answer that it is not configured. Set, with
//! `AGENT_MODEL_PROVIDER` unset: the module holds its tables — so an erasure
//! or a purge still reaches rows written while it was on — and answers the
//! same. Both set: the agent is on, and the embeddings endpoint is needed too.

use telmoni_shared::Redacted;
use telmoni_shared::config::{env_parse, optional, optional_base_url, require};

/// The model the docs name as the default for the Anthropic protocol.
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-5";

/// The embedding model the column is sized for, served by a local Ollama.
pub const DEFAULT_EMBEDDINGS_MODEL: &str = "nomic-embed-text";

/// The width of `agent.chunks.embedding`. An embedding model of another
/// width is refused at boot (`boot::probe_width`), since pgvector would
/// otherwise refuse every write, or a truncated vector would search nonsense.
pub const EMBEDDING_DIMENSIONS: usize = 768;

/// The port `telmoni-server serve` listens on when `PORT` is unset: the binary's
/// own default, spelled here too because this crate cannot read it.
const DEFAULT_PORT: &str = "8082";

/// The SDK's configuration for the agent's own runs, read as the SDK reads
/// it — `TELMONI_ENDPOINT`, `TELMONI_API_KEY`, `TELMONI_ORG` — with one
/// difference from a customer's: with no endpoint set, the runs go to this
/// server's own address, `http://127.0.0.1:$PORT`, never to the SDK's default
/// of the hosted service, so a server never calls another on its own.
/// `endpoint` and `port` are the raw variables; `sdk` is what the SDK read.
fn reporting(
    mut sdk: telmoni::Config,
    endpoint: Option<&str>,
    port: Option<&str>,
) -> anyhow::Result<telmoni::Config> {
    if endpoint.is_none() {
        sdk.endpoint = format!("http://127.0.0.1:{}", port.unwrap_or(DEFAULT_PORT));
    }
    reqwest::Url::parse(&sdk.endpoint)
        .map_err(|e| anyhow::anyhow!("TELMONI_ENDPOINT is not a URL: {e}"))?;
    Ok(sdk)
}

/// The docs corpus when `DOCS_CORPUS_URL` is unset: the console's own
/// `/llms-full.txt` under `APP_URL`, every page of the docs at the version
/// this deployment runs. The compose file and the chart name the console
/// service directly instead, so the fetch stays inside the deployment.
fn default_docs_corpus_url() -> anyhow::Result<String> {
    Ok(format!(
        "{}/llms-full.txt",
        telmoni_shared::config::app_url()?
    ))
}

/// Which wire protocol the model endpoint speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// Anthropic's Messages API.
    Anthropic,
    /// OpenAI's Chat Completions, which OpenAI, Gemini's compatible
    /// endpoint, Ollama, vLLM, OpenRouter and LM Studio all serve.
    OpenAi,
}

impl Provider {
    /// The name `AGENT_MODEL_PROVIDER` takes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        }
    }
}

impl std::str::FromStr for Provider {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        [Self::Anthropic, Self::OpenAi]
            .into_iter()
            .find(|provider| provider.as_str() == s)
            .ok_or_else(|| {
                anyhow::anyhow!("AGENT_MODEL_PROVIDER must be `anthropic` or `openai`, got {s:?}")
            })
    }
}

/// The chat model.
#[derive(Clone, Debug)]
pub struct ModelConfig {
    pub provider: Provider,
    /// The API's base (`AGENT_MODEL_URL`): `https://api.anthropic.com` for
    /// Anthropic, and the `/v1` base for the other protocol
    /// (`http://localhost:11434/v1` for a local Ollama).
    pub url: String,
    /// `AGENT_MODEL`.
    pub model: String,
    /// `AGENT_MODEL_API_KEY`; a local server needs none.
    pub api_key: Option<Redacted>,
    /// The longest reply one model call may write.
    pub max_tokens: u32,
    /// How long one model call may go without sending anything: before its
    /// first byte, or between two pieces of its answer.
    pub timeout_secs: u64,
}

/// An OpenAI-compatible `/embeddings` endpoint.
#[derive(Clone, Debug)]
pub struct EmbeddingsConfig {
    /// `EMBEDDINGS_URL`, the `/v1` base.
    pub url: String,
    /// `EMBEDDINGS_MODEL`.
    pub model: String,
    /// `EMBEDDINGS_API_KEY`.
    pub api_key: Option<Redacted>,
    /// `EMBEDDINGS_REQUEST_DIMENSIONS`: send `dimensions: 768`, for a
    /// hosted model that is wider by default and shortens on request
    /// (OpenAI's `text-embedding-3-*`, Gemini's). Off for a model that is
    /// 768 wide already, some of whose servers refuse the field.
    pub request_dimensions: bool,
}

/// A rerank endpoint (`POST {RERANK_URL}` taking `{model, query, documents}`
/// and answering `{results: [{index, relevance_score}]}`, as Cohere, Jina
/// and llama.cpp's server do).
#[derive(Clone, Debug)]
pub struct RerankConfig {
    pub url: String,
    pub model: String,
    pub api_key: Option<Redacted>,
}

/// Loaded-once agent configuration.
#[derive(Clone, Debug)]
pub struct Config {
    /// `AGENT_DATABASE_URL`, opened as the `agent` role.
    pub database_url: String,
    /// `None` when `AGENT_MODEL_PROVIDER` is unset: the agent is off.
    pub model: Option<ModelConfig>,
    pub embeddings: EmbeddingsConfig,
    pub rerank: Option<RerankConfig>,
    /// `DOCS_CORPUS_URL`: unset, the console's own corpus; `off` indexes no
    /// docs.
    pub docs_corpus_url: Option<String>,
    /// `AGENT_MESSAGES_PER_HOUR`: the questions one person may ask in an hour.
    pub messages_per_hour: i64,
    /// The SDK, reporting each question as a run of the operator's own
    /// project the way any agent reports its runs (`report.rs`): on while
    /// `TELMONI_API_KEY` holds an ingest key of that project; unset, nothing
    /// records them.
    pub telmoni: Option<telmoni::Config>,
}

impl Config {
    /// The configuration, or `None` when the deployment has no agent.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(database_url) = optional("AGENT_DATABASE_URL") else {
            if optional("AGENT_MODEL_PROVIDER").is_some() {
                anyhow::bail!(
                    "AGENT_MODEL_PROVIDER is set but AGENT_DATABASE_URL is not; the agent keeps \
                     its conversations and its index in its own schema"
                );
            }
            return Ok(None);
        };
        let model = match optional("AGENT_MODEL_PROVIDER") {
            None => None,
            Some(raw) => Some(model_from_env(raw.parse()?)?),
        };
        let docs_corpus_url = match optional_base_url("DOCS_CORPUS_URL") {
            Some(off) if off == "off" => None,
            Some(url) => Some(url),
            None => Some(default_docs_corpus_url()?),
        };
        let telmoni = match optional("TELMONI_API_KEY") {
            None => None,
            Some(_) => Some(reporting(
                telmoni::Config::from_env(),
                optional("TELMONI_ENDPOINT").as_deref(),
                optional("PORT").as_deref(),
            )?),
        };
        let config = Self {
            database_url,
            model,
            embeddings: EmbeddingsConfig {
                url: optional_base_url("EMBEDDINGS_URL")
                    .unwrap_or_else(|| "http://localhost:11434/v1".to_owned()),
                model: optional("EMBEDDINGS_MODEL")
                    .unwrap_or_else(|| DEFAULT_EMBEDDINGS_MODEL.to_owned()),
                api_key: optional("EMBEDDINGS_API_KEY").map(Into::into),
                request_dimensions: env_parse("EMBEDDINGS_REQUEST_DIMENSIONS", false)?,
            },
            rerank: match optional_base_url("RERANK_URL") {
                None => None,
                Some(url) => Some(RerankConfig {
                    url,
                    model: require("RERANK_MODEL")?,
                    api_key: optional("RERANK_API_KEY").map(Into::into),
                }),
            },
            docs_corpus_url,
            messages_per_hour: env_parse("AGENT_MESSAGES_PER_HOUR", 60)?,
            telmoni,
        };
        if config.messages_per_hour < 1 {
            anyhow::bail!("AGENT_MESSAGES_PER_HOUR must be at least 1");
        }
        if config.enabled() && config.embeddings.api_key.is_none() && hosted(&config.embeddings.url)
        {
            anyhow::bail!(
                "EMBEDDINGS_URL is a hosted API ({}) but EMBEDDINGS_API_KEY is not set",
                config.embeddings.url
            );
        }
        Ok(Some(config))
    }

    /// Whether a person can ask the agent anything.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.model.is_some()
    }
}

fn model_from_env(provider: Provider) -> anyhow::Result<ModelConfig> {
    let (url, model) = match provider {
        Provider::Anthropic => (
            optional_base_url("AGENT_MODEL_URL")
                .unwrap_or_else(|| "https://api.anthropic.com".to_owned()),
            optional("AGENT_MODEL").unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_owned()),
        ),
        // No default model: "the OpenAI protocol" names a dozen servers, and
        // a guessed model name is a 404 at the first question.
        Provider::OpenAi => (
            optional_base_url("AGENT_MODEL_URL")
                .unwrap_or_else(|| "https://api.openai.com/v1".to_owned()),
            require("AGENT_MODEL")?,
        ),
    };
    let config = ModelConfig {
        provider,
        url,
        model,
        api_key: optional("AGENT_MODEL_API_KEY").map(Into::into),
        max_tokens: env_parse("AGENT_MAX_TOKENS", 8192)?,
        timeout_secs: env_parse("AGENT_MODEL_TIMEOUT_SECS", 60)?,
    };
    if config.api_key.is_none() && hosted(&config.url) {
        anyhow::bail!(
            "AGENT_MODEL_URL is a hosted API ({}) but AGENT_MODEL_API_KEY is not set",
            config.url
        );
    }
    if config.max_tokens < 1 {
        anyhow::bail!("AGENT_MAX_TOKENS must be at least 1");
    }
    // A longer call would be cut by the turn's own budget anyway, so a value
    // past it is a setting that does nothing.
    let budget = crate::turn::TURN_BUDGET.as_secs();
    if !(1..=budget).contains(&config.timeout_secs) {
        anyhow::bail!("AGENT_MODEL_TIMEOUT_SECS must be between 1 and {budget}");
    }
    Ok(config)
}

impl ModelConfig {
    /// The provider its calls are recorded under, as the GenAI conventions
    /// name it: the vendor its URL is, else its protocol, so a self-hosted
    /// model is priced only under a name that protocol's vendor lists.
    #[must_use]
    pub fn provider_name(&self) -> &'static str {
        vendor(&self.url).unwrap_or_else(|| self.provider.as_str())
    }
}

/// The vendors' APIs, by host, each under the GenAI conventions' name for it.
const VENDORS: [(&str, &str); 3] = [
    ("api.anthropic.com", "anthropic"),
    ("api.openai.com", "openai"),
    ("generativelanguage.googleapis.com", "gcp.gemini"),
];

/// The vendor whose API `url` is, by its host whole, so a name that merely
/// begins like one is no vendor's.
fn vendor(url: &str) -> Option<&'static str> {
    let url = reqwest::Url::parse(url).ok()?;
    let host = url.host_str()?;
    VENDORS
        .iter()
        .find_map(|&(known, name)| (known == host).then_some(name))
}

/// A vendor's API that answers nothing without a key. A local or self-hosted
/// server under the same protocol may need none, so only these are checked.
fn hosted(url: &str) -> bool {
    vendor(url).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_protocols_parse() {
        assert_eq!(
            "anthropic".parse::<Provider>().unwrap(),
            Provider::Anthropic
        );
        assert_eq!("openai".parse::<Provider>().unwrap(), Provider::OpenAi);
        assert!("gemini".parse::<Provider>().is_err());
    }

    /// With no endpoint set, the agent's runs go to this server's own port,
    /// never to the SDK's default; an endpoint set is kept, and one that is
    /// no URL refuses boot.
    #[test]
    fn the_runs_go_to_this_server_unless_an_endpoint_is_set() {
        let sdk = || telmoni::Config::new("", None, Some("tm_key".into()));
        assert_eq!(sdk().endpoint, telmoni::DEFAULT_ENDPOINT);
        let own = reporting(sdk(), None, Some("8083")).unwrap();
        assert_eq!(own.endpoint, "http://127.0.0.1:8083");
        let default_port = reporting(sdk(), None, None).unwrap();
        assert_eq!(default_port.endpoint, "http://127.0.0.1:8082");
        let set = reporting(
            telmoni::Config::new("https://api.example.com/", None, Some("tm_key".into())),
            Some("https://api.example.com/"),
            None,
        )
        .unwrap();
        assert_eq!(set.endpoint, "https://api.example.com");
        assert_eq!(own.api_key.as_deref(), Some("tm_key"));
        let bad = telmoni::Config {
            endpoint: "not a url".into(),
            ..sdk()
        };
        assert!(reporting(bad, Some("not a url"), None).is_err());
    }

    #[test]
    fn only_a_vendor_api_is_held_to_a_key() {
        assert!(hosted("https://api.anthropic.com"));
        assert!(hosted("https://api.openai.com/v1"));
        assert!(hosted(
            "https://generativelanguage.googleapis.com/v1beta/openai"
        ));
        assert!(!hosted("http://localhost:11434/v1"));
        assert!(!hosted("https://llm.internal.example.com/v1"));
        assert!(!hosted("https://api.openai.com.example.net/v1"));
    }

    /// ⚠ A call is recorded under the vendor its URL is, the price table's name for it.
    #[test]
    fn a_call_is_named_by_the_vendor_its_url_is_else_its_protocol() {
        let config = |provider, url: &str| ModelConfig {
            provider,
            url: url.to_owned(),
            model: "m".to_owned(),
            api_key: None,
            max_tokens: 8192,
            timeout_secs: 60,
        };
        for (provider, url, name) in [
            (
                Provider::Anthropic,
                "https://api.anthropic.com",
                "anthropic",
            ),
            (Provider::OpenAi, "https://api.openai.com/v1", "openai"),
            (
                Provider::OpenAi,
                "https://generativelanguage.googleapis.com/v1beta/openai",
                "gcp.gemini",
            ),
            (Provider::OpenAi, "https://API.OpenAI.com:443/v1", "openai"),
            (
                Provider::OpenAi,
                "https://api.anthropic.com/v1",
                "anthropic",
            ),
            (Provider::OpenAi, "http://localhost:11434/v1", "openai"),
            (Provider::OpenAi, "https://openrouter.ai/api/v1", "openai"),
            (
                Provider::OpenAi,
                "https://generativelanguage.googleapis.com.example.net/v1",
                "openai",
            ),
            (
                Provider::Anthropic,
                "https://llm.internal.example.com",
                "anthropic",
            ),
            (Provider::OpenAi, "not a url", "openai"),
        ] {
            assert_eq!(config(provider, url).provider_name(), name, "{url}");
        }
    }
}
