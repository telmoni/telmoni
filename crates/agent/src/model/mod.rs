//! The chat model, behind one trait and two wire protocols.
//!
//! [`anthropic`] speaks the Messages API and [`openai`] speaks Chat
//! Completions, which covers every other vendor and every local server worth
//! running. Both stream: text reaches the person as the model writes it,
//! through the `on_text` sink a call is handed.
//!
//! ⚠ **Not `Egress::guarded`.** The guard refuses private and link-local
//! addresses, and it exists for URLs a customer names. These URLs are the
//! operator's (`AGENT_MODEL_URL`, `EMBEDDINGS_URL`, `RERANK_URL`), and a local
//! Ollama on a private address is exactly what a self-hosted tier points
//! them at.

pub mod anthropic;
pub mod openai;

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use telmoni_shared::TelmoniError;
pub use telmoni_shared::seam::TokenUsage;

use crate::config::{ModelConfig, Provider};

/// A tool the model may call.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The input's JSON Schema.
    pub parameters: Value,
}

/// One call the model asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// The provider's id, which the result must answer.
    pub id: String,
    pub name: String,
    /// The arguments, parsed; `Value::Null` when they were not valid JSON,
    /// which the tool answers as an error rather than guessing.
    pub input: Value,
}

/// One tool's answer, as it goes back to the model.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub id: String,
    pub content: String,
    pub is_error: bool,
}

/// The transcript, in neither provider's shape.
#[derive(Debug, Clone)]
pub enum Message {
    User(String),
    Assistant {
        text: String,
        tool_calls: Vec<ToolCall>,
        /// The provider's own content blocks for this turn, sent back
        /// verbatim when there are any: a thinking block must return
        /// unchanged, signature and all, or the next call is refused.
        raw: Option<Value>,
    },
    ToolResults(Vec<ToolResult>),
}

/// Why a call stopped, in neither provider's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StopReason {
    /// The model finished what it was writing.
    Finished,
    /// It wants its tool calls run.
    ToolUse,
    /// It ran into `AGENT_MAX_TOKENS` mid-reply.
    MaxTokens,
    /// It declined, or the provider's filter stopped it.
    Refused,
    /// A reason neither protocol documents; treated as finished.
    Other,
    /// The stream ended without saying why: the connection closed before
    /// the provider's last event, or the body was not a stream at all. Where
    /// every call starts, so only a stream that says it finished is one.
    #[default]
    Unfinished,
}

impl StopReason {
    /// The Messages API's `stop_reason`.
    #[must_use]
    pub fn from_anthropic(reason: &str) -> Self {
        match reason {
            "end_turn" | "stop_sequence" => Self::Finished,
            "tool_use" => Self::ToolUse,
            "max_tokens" | "model_context_window_exceeded" => Self::MaxTokens,
            "refusal" => Self::Refused,
            _ => Self::Other,
        }
    }

    /// Chat Completions' `finish_reason`.
    #[must_use]
    pub fn from_openai(reason: &str) -> Self {
        match reason {
            "stop" => Self::Finished,
            "tool_calls" | "function_call" => Self::ToolUse,
            "length" => Self::MaxTokens,
            "content_filter" => Self::Refused,
            _ => Self::Other,
        }
    }

    /// The reason as a question's record names it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Finished => "finished",
            Self::ToolUse => "tool_use",
            Self::MaxTokens => "max_tokens",
            Self::Refused => "refused",
            Self::Other => "other",
            Self::Unfinished => "unfinished",
        }
    }
}

/// What one call came to.
#[derive(Debug, Clone, Default)]
pub struct ModelTurn {
    /// Every text delta, joined.
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    /// See [`Message::Assistant::raw`].
    pub raw: Option<Value>,
    pub stop_reason: StopReason,
    /// The tokens the provider said the call used; `None` when it said
    /// nothing, as some local servers do.
    pub usage: Option<TokenUsage>,
    /// The model that answered as the provider spells it, where the stream
    /// names one.
    pub model: Option<String>,
}

impl ModelTurn {
    /// The turn as the next call's transcript carries it.
    #[must_use]
    pub fn into_message(self) -> Message {
        Message::Assistant {
            text: self.text,
            tool_calls: self.tool_calls,
            raw: self.raw,
        }
    }
}

/// Where the text goes as it streams.
pub type TextSink<'a> = &'a (dyn Fn(&str) + Send + Sync);

/// A chat model that streams.
#[async_trait]
pub trait Model: Send + Sync {
    /// One call: the system prompt, the transcript and the tools, with each
    /// text delta handed to `on_text` as it arrives.
    async fn stream(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        on_text: TextSink<'_>,
    ) -> Result<ModelTurn, TelmoniError>;
}

/// A client for one of the operator's endpoints. `timeout` bounds a whole
/// call, the streamed body included. Redirects are refused: an endpoint
/// that moves is a configuration to fix, and a redirect would carry the key
/// to wherever it pointed.
pub(crate) fn http_client(timeout: std::time::Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("telmoni-agent")
        .build()
}

/// A client for a streaming model call. `stall` bounds the wait for the
/// next piece of the answer, not the whole of it: a long answer streaming
/// steadily is not a hung one, and cutting it at a fixed total threw away
/// what the person had already read. The turn's budget bounds the whole.
fn stream_client(stall: std::time::Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(stall)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("telmoni-agent")
        .build()
}

/// How long a model endpoint may take to accept the connection.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The model the configuration names.
pub fn from_config(config: &ModelConfig) -> anyhow::Result<Arc<dyn Model>> {
    let http = stream_client(std::time::Duration::from_secs(config.timeout_secs))?;
    Ok(match config.provider {
        Provider::Anthropic => Arc::new(anthropic::Anthropic::new(http, config.clone())),
        Provider::OpenAi => Arc::new(openai::OpenAi::new(http, config.clone())),
    })
}

/// A model failure as the person sees it — a 502 whose cause stays in the
/// log. Never the provider's body: a refusal can echo the request back.
pub(crate) fn unavailable(context: impl Into<String>) -> TelmoniError {
    TelmoniError::AgentModelUnavailable {
        context: context.into(),
    }
}

/// A non-2xx answer: the status, and the error's code when the body names
/// one, never its sentence. OpenAI and Anthropic say `type` or `code`;
/// Google says `status` (`UNAVAILABLE`, `NOT_FOUND`), inside a one-element
/// array on its OpenAI-compatible endpoint. One that says "not now" is
/// marked so ([`not_now`]).
pub(crate) async fn refused(provider: &str, response: reqwest::Response) -> TelmoniError {
    let status = response.status().as_u16();
    let kind = response
        .json::<Value>()
        .await
        .ok()
        .and_then(|body| error_code(&body))
        .unwrap_or_default();
    let answered = format!("answered {status} {kind}");
    if NOT_NOW.contains(&status) {
        not_now_error(provider, &answered)
    } else {
        unavailable(format!("{provider} {answered}"))
    }
}

/// Answers that say "not now" whatever was sent: a timeout, a rate limit,
/// an endpoint saying it is unavailable or overloaded (Anthropic's 529). A
/// 500, 502 or 504 can be a backend failing on the one input it was sent,
/// so none of them is among these.
const NOT_NOW: [u16; 4] = [408, 429, 503, 529];

/// How a failure that is the endpoint's state, never what it was sent, is
/// marked, for [`not_now`].
const NOT_NOW_MARK: &str = "not now";

/// A failure that is the endpoint's state, never what it was sent: no
/// answer in time, no connection, a body that never arrived whole, or a
/// [`NOT_NOW`] answer.
pub(crate) fn not_now_error(provider: &str, detail: &str) -> TelmoniError {
    unavailable(format!("{provider} {NOT_NOW_MARK}: {detail}"))
}

/// Whether an error is the endpoint's state rather than anything about the
/// request: asking again later cures it, and sending something else proves
/// nothing. The context opens on the provider's one-word name, then the
/// mark, then `": "`; an error code the provider chose comes later, so it
/// cannot read as one.
pub(crate) fn not_now(error: &TelmoniError) -> bool {
    not_now_detail(error).is_some()
}

/// Whether an error is the endpoint's rate limit — a 429, after [`send`]'s
/// retries — which waiting out its window may lift.
pub(crate) fn rate_limited(error: &TelmoniError) -> bool {
    not_now_detail(error).is_some_and(|detail| detail.starts_with("answered 429 "))
}

/// What a "not now" error says after its mark, read only at its head.
fn not_now_detail(error: &TelmoniError) -> Option<&str> {
    let TelmoniError::AgentModelUnavailable { context } = error else {
        return None;
    };
    let (head, detail) = context.split_once(": ")?;
    head.split_once(' ')
        .is_some_and(|(_, mark)| mark == NOT_NOW_MARK)
        .then_some(detail)
}

fn error_code(body: &Value) -> Option<String> {
    let error = body.get("error").or_else(|| body.pointer("/0/error"))?;
    ["type", "code", "status"]
        .iter()
        .find_map(|key| error.get(key).and_then(Value::as_str))
        .map(|s| s.chars().take(64).collect())
}

/// Answers that mean "not now" rather than "not this": a timeout, a rate
/// limit, and an upstream that is overloaded or restarting (Anthropic's 529,
/// and Cloudflare's 520–524 in front of a provider, among them).
const BUSY: [u16; 11] = [408, 429, 502, 503, 504, 520, 521, 522, 523, 524, 529];

/// How many times a busy endpoint is asked again before the person hears.
const RETRIES: u32 = 2;

/// The longest `retry-after` honoured: a longer one outlasts the turn.
const MAX_PAUSE: std::time::Duration = std::time::Duration::from_secs(5);

/// Send `request`, asking again after a pause while the endpoint answers
/// busy. Repeating is safe: a refusal arrives before any body, so nothing
/// has streamed to the person yet. The turn's deadline still bounds it all.
pub(crate) async fn send(
    provider: &str,
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, TelmoniError> {
    let mut attempt = 0;
    loop {
        let this = request
            .try_clone()
            .ok_or_else(|| unavailable(format!("{provider} request cannot be repeated")))?;
        let response = this
            .send()
            .await
            .map_err(|e| not_now_error(provider, error_kind(&e)))?;
        let status = response.status().as_u16();
        if response.status().is_success() {
            return Ok(response);
        }
        if attempt == RETRIES || !BUSY.contains(&status) {
            return Err(refused(provider, response).await);
        }
        attempt += 1;
        let wait = pause(
            attempt,
            response.headers().get(reqwest::header::RETRY_AFTER),
        );
        tracing::info!(
            provider,
            status,
            attempt,
            "agent endpoint busy; asking again"
        );
        tokio::time::sleep(wait).await;
    }
}

/// The endpoint's own `retry-after` in seconds when it gives one, else 1s
/// then 2s; capped either way.
fn pause(attempt: u32, retry_after: Option<&reqwest::header::HeaderValue>) -> std::time::Duration {
    retry_after
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map_or_else(
            || std::time::Duration::from_secs(1 << (attempt - 1)),
            std::time::Duration::from_secs,
        )
        .min(MAX_PAUSE)
}

/// Server-sent events, fed whatever byte chunks the connection delivers and
/// handing back each whole event: its `event:` name, when it has one, and
/// its `data:` lines joined. Comments and unknown fields are dropped.
#[derive(Debug, Default)]
pub(crate) struct SseReader {
    buffer: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

/// One event off the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

impl SseReader {
    /// Take a chunk; answer every event it completed.
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() || self.event.is_some() {
                    events.push(SseEvent {
                        event: self.event.take(),
                        data: std::mem::take(&mut self.data).join("\n"),
                    });
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => self.event = Some(value.to_owned()),
                "data" => self.data.push(value.to_owned()),
                _ => {}
            }
        }
        events
    }
}

/// Read a streaming response to its end, handing each event to `on_event`,
/// which answers `false` to stop early.
pub(crate) async fn read_events(
    provider: &str,
    mut response: reqwest::Response,
    mut on_event: impl FnMut(SseEvent) -> Result<bool, TelmoniError>,
) -> Result<(), TelmoniError> {
    let mut reader = SseReader::default();
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|e| unavailable(format!("{provider} stream broke: {}", error_kind(&e))))?;
        let Some(chunk) = chunk else {
            return Ok(());
        };
        for event in reader.feed(&chunk) {
            if !on_event(event)? {
                return Ok(());
            }
        }
    }
}

/// What kind of transport failure, without the URL reqwest would print —
/// a query string can carry a key.
pub(crate) fn error_kind(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "could not connect"
    } else if e.is_body() || e.is_decode() {
        "bad body"
    } else if e.is_request() {
        "request failed"
    } else {
        "failed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_split_across_chunks_come_out_whole() {
        let mut reader = SseReader::default();
        assert!(reader.feed(b"event: text\nda").is_empty());
        assert!(reader.feed(b"ta: {\"a\":1}\r\n").is_empty());
        let out = reader.feed(b"\r\n: keepalive\n\ndata: [DONE]\n\n");
        assert_eq!(
            out,
            vec![
                SseEvent {
                    event: Some("text".into()),
                    data: "{\"a\":1}".into()
                },
                SseEvent {
                    event: None,
                    data: "[DONE]".into()
                },
            ]
        );
    }

    #[test]
    fn multi_line_data_is_joined() {
        let mut reader = SseReader::default();
        let out = reader.feed(b"data: one\ndata: two\n\n");
        assert_eq!(out.first().map(|e| e.data.as_str()), Some("one\ntwo"));
    }

    #[test]
    fn every_vendors_error_code_is_read() {
        let openai = serde_json::json!({"error": {"type": "invalid_request_error"}});
        let google = serde_json::json!([{"error": {"code": 503, "status": "UNAVAILABLE"}}]);
        assert_eq!(
            error_code(&openai).as_deref(),
            Some("invalid_request_error")
        );
        assert_eq!(error_code(&google).as_deref(), Some("UNAVAILABLE"));
        assert_eq!(error_code(&serde_json::json!("nope")), None);
    }

    #[test]
    fn the_pause_backs_off_and_is_capped() {
        use reqwest::header::HeaderValue;
        assert_eq!(pause(1, None), std::time::Duration::from_secs(1));
        assert_eq!(pause(2, None), std::time::Duration::from_secs(2));
        assert_eq!(
            pause(1, Some(&HeaderValue::from_static("0"))),
            std::time::Duration::ZERO
        );
        assert_eq!(pause(1, Some(&HeaderValue::from_static("600"))), MAX_PAUSE);
    }

    async fn post(server: &wiremock::MockServer) -> Result<reqwest::Response, TelmoniError> {
        let http = http_client(std::time::Duration::from_secs(5)).unwrap();
        send("test", http.post(server.uri()).json(&serde_json::json!({}))).await
    }

    fn busy() -> wiremock::ResponseTemplate {
        wiremock::ResponseTemplate::new(503).insert_header("retry-after", "0")
    }

    #[tokio::test]
    async fn a_busy_endpoint_is_asked_again() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(busy())
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        assert!(post(&server).await.is_ok());
    }

    #[tokio::test]
    async fn a_refusal_is_not_repeated_and_busy_gives_up() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        assert!(post(&server).await.is_err());
        drop(server);

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(busy())
            .expect(u64::from(RETRIES) + 1)
            .mount(&server)
            .await;
        let err = post(&server).await.unwrap_err();
        assert!(err.to_string().contains("503"), "{err}");
        assert!(not_now(&err), "a 503 is the endpoint's state: {err}");
    }

    /// Only the mark at the head reads as "not now": a code the provider
    /// chose, later in the context, cannot pass for it.
    #[test]
    fn only_the_endpoints_state_reads_as_not_now() {
        assert!(not_now(&not_now_error("embeddings", "timed out")));
        assert!(!not_now(&unavailable(
            "embeddings answered 500 x not now: y"
        )));
        assert!(!not_now(&unavailable(
            "embeddings answered 500 server_error"
        )));
        assert!(rate_limited(&not_now_error(
            "embeddings",
            "answered 429 rate_limit_exceeded"
        )));
        assert!(!rate_limited(&not_now_error("embeddings", "answered 503 ")));
        assert!(!rate_limited(&unavailable(
            "embeddings answered 500 not now: answered 429 x"
        )));
    }

    #[tokio::test]
    async fn a_failure_the_input_could_cause_is_not_marked_not_now() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        for (status, endpoint) in [(429, true), (500, false), (502, false), (413, false)] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(status).insert_header("retry-after", "0"))
                .mount(&server)
                .await;
            let err = post(&server).await.unwrap_err();
            assert_eq!(not_now(&err), endpoint, "{status}: {err}");
        }
    }
}
