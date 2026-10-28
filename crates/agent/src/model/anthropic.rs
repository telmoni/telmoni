//! The Messages API, streaming, with tool use.
//!
//! Each content block is rebuilt from its deltas and the whole turn is kept
//! as `raw`, so a thinking block goes back exactly as it came — a model that
//! thinks by default refuses a transcript whose thinking was altered.

use std::borrow::Cow;
use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use telmoni_shared::TelmoniError;

use super::{
    Assemble, Endpoint, Flow, Message, Model, ModelTurn, SseEvent, StopReason, TextSink,
    TokenUsage, ToolCall, ToolSpec, stream_turn, tool_input, unavailable,
};
use crate::config::ModelConfig;

const API_VERSION: &str = "2023-06-01";

pub struct Anthropic {
    http: reqwest::Client,
    config: ModelConfig,
}

impl Anthropic {
    #[must_use]
    pub const fn new(http: reqwest::Client, config: ModelConfig) -> Self {
        Self { http, config }
    }
}

/// One message of the transcript in the Messages API's shape.
#[derive(Serialize)]
pub(crate) struct Turn<'a> {
    role: &'static str,
    content: Content<'a>,
}

/// A message's content: text, blocks, or the provider's own blocks back
/// verbatim.
#[derive(Serialize)]
#[serde(untagged)]
enum Content<'a> {
    Text(&'a str),
    Blocks(Vec<BlockOut<'a>>),
    Raw(&'a Value),
}

/// One content block as the request carries it.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BlockOut<'a> {
    Text {
        text: &'a str,
    },
    ToolUse {
        id: &'a str,
        name: &'a str,
        input: Cow<'a, Value>,
    },
    ToolResult {
        tool_use_id: &'a str,
        content: &'a str,
        is_error: bool,
    },
}

/// The transcript in the Messages API's shape.
pub(crate) fn messages(transcript: &[Message]) -> Vec<Turn<'_>> {
    transcript
        .iter()
        .map(|message| match message {
            Message::User(text) => Turn {
                role: "user",
                content: Content::Text(text),
            },
            Message::Assistant { raw: Some(raw), .. } => Turn {
                role: "assistant",
                content: Content::Raw(raw),
            },
            Message::Assistant {
                text, tool_calls, ..
            } => {
                let mut content = Vec::new();
                if !text.is_empty() {
                    content.push(BlockOut::Text { text });
                }
                for call in tool_calls {
                    content.push(BlockOut::ToolUse {
                        id: &call.id,
                        name: &call.name,
                        input: if call.input.is_object() {
                            Cow::Borrowed(&call.input)
                        } else {
                            Cow::Owned(json!({}))
                        },
                    });
                }
                Turn {
                    role: "assistant",
                    content: Content::Blocks(content),
                }
            }
            Message::ToolResults(results) => Turn {
                role: "user",
                content: Content::Blocks(
                    results
                        .iter()
                        .map(|r| BlockOut::ToolResult {
                            tool_use_id: &r.id,
                            content: &r.content,
                            is_error: r.is_error,
                        })
                        .collect(),
                ),
            },
        })
        .collect()
}

/// One streamed event, by its `type`; one the API adds later is `Other`.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event<'a> {
    MessageStart {
        message: Start,
    },
    ContentBlockStart {
        #[serde(default)]
        index: u64,
        content_block: Value,
    },
    ContentBlockDelta {
        #[serde(default)]
        index: u64,
        #[serde(borrow)]
        delta: Delta<'a>,
    },
    MessageDelta {
        #[serde(borrow)]
        delta: Stop<'a>,
        #[serde(default)]
        usage: Option<Counted>,
    },
    MessageStop,
    Error {
        #[serde(borrow, default)]
        error: Option<Failure<'a>>,
    },
    #[serde(other)]
    Other,
}

/// `message_start`'s message: the model answering, and the input's counts.
#[derive(Deserialize)]
struct Start {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<Counted>,
}

/// A usage block as the stream gives it, each count where it is given.
#[derive(Deserialize)]
struct Counted {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

/// A block's delta, by its `type`.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum Delta<'a> {
    #[serde(rename = "text_delta")]
    Text {
        #[serde(borrow)]
        text: Cow<'a, str>,
    },
    #[serde(rename = "input_json_delta")]
    InputJson {
        #[serde(borrow)]
        partial_json: Cow<'a, str>,
    },
    #[serde(rename = "thinking_delta")]
    Thinking {
        #[serde(borrow)]
        thinking: Cow<'a, str>,
    },
    #[serde(rename = "signature_delta")]
    Signature {
        #[serde(borrow)]
        signature: Cow<'a, str>,
    },
    #[serde(other)]
    Other,
}

/// `message_delta`'s delta: why the model stopped, once it has.
#[derive(Deserialize)]
struct Stop<'a> {
    #[serde(borrow, default)]
    stop_reason: Option<Cow<'a, str>>,
}

/// An `error` event's error: its type, which is all the log says of it.
#[derive(Deserialize)]
struct Failure<'a> {
    #[serde(borrow, default, rename = "type")]
    kind: Option<Cow<'a, str>>,
}

/// One content block as its deltas build it.
#[derive(Debug)]
enum Block {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    /// Anything else, kept as the start event gave it.
    Other(Value),
}

impl Block {
    fn start(block: &Value) -> Self {
        let text = |key: &str| {
            block
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        match block.get("type").and_then(Value::as_str) {
            Some("text") => Self::Text(text("text")),
            Some("tool_use") => Self::ToolUse {
                id: text("id"),
                name: text("name"),
                json: String::new(),
            },
            Some("thinking") => Self::Thinking {
                thinking: text("thinking"),
                signature: text("signature"),
            },
            _ => Self::Other(block.clone()),
        }
    }

    fn into_raw(self) -> Value {
        match self {
            Self::Text(text) => json!({ "type": "text", "text": text }),
            Self::ToolUse { id, name, json } => {
                let input = tool_input(&json);
                json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": if input.is_object() { input } else { json!({}) },
                })
            }
            Self::Thinking {
                thinking,
                signature,
            } => json!({ "type": "thinking", "thinking": thinking, "signature": signature }),
            Self::Other(value) => value,
        }
    }
}

/// The streamed events of one turn, folded into its blocks.
#[derive(Debug, Default)]
pub(crate) struct Assembler {
    blocks: BTreeMap<u64, Block>,
    stop_reason: StopReason,
    counts: Counts,
    model: Option<String>,
}

/// The Messages API's token counts as the stream gives them: `message_start`
/// carries the input's, `message_delta` the output's totals so far. Each
/// count read replaces the last.
#[derive(Debug, Default)]
struct Counts {
    input: Option<u64>,
    cache_read: u64,
    cache_write: u64,
    output: u64,
}

impl Counts {
    fn take(&mut self, usage: &Counted) {
        if let Some(input) = usage.input_tokens {
            self.input = Some(input);
        }
        if let Some(read) = usage.cache_read_input_tokens {
            self.cache_read = read;
        }
        if let Some(write) = usage.cache_creation_input_tokens {
            self.cache_write = write;
        }
        if let Some(output) = usage.output_tokens {
            self.output = output;
        }
    }

    /// The API counts its cache apart from `input_tokens`; the record counts
    /// it among the input, as the conventions do. Thinking is billed as
    /// output and counted in it, with no count of its own.
    fn usage(&self) -> Option<TokenUsage> {
        Some(TokenUsage {
            input_tokens: self.input? + self.cache_read + self.cache_write,
            output_tokens: self.output,
            cache_read_tokens: self.cache_read,
            cache_write_tokens: self.cache_write,
            reasoning_tokens: 0,
        })
    }
}

impl Assemble for Assembler {
    const ENDPOINT: Endpoint = Endpoint::Anthropic;

    fn take(&mut self, event: &SseEvent) -> Result<Flow, TelmoniError> {
        let Ok(event) = serde_json::from_str::<Event<'_>>(&event.data) else {
            return Ok(Flow::Continue);
        };
        match event {
            Event::ContentBlockStart {
                index,
                content_block,
            } => {
                let block = Block::start(&content_block);
                let initial = match &block {
                    Block::Text(text) if !text.is_empty() => Some(text.clone()),
                    _ => None,
                };
                self.blocks.insert(index, block);
                return Ok(initial.map_or(Flow::Continue, Flow::Text));
            }
            Event::ContentBlockDelta { index, delta } => {
                match (self.blocks.get_mut(&index), delta) {
                    (Some(Block::Text(text)), Delta::Text { text: added }) => {
                        text.push_str(&added);
                        return Ok(Flow::Text(added.into_owned()));
                    }
                    (Some(Block::ToolUse { json, .. }), Delta::InputJson { partial_json }) => {
                        json.push_str(&partial_json);
                    }
                    (
                        Some(Block::Thinking { thinking, .. }),
                        Delta::Thinking { thinking: added },
                    ) => thinking.push_str(&added),
                    (
                        Some(Block::Thinking { signature, .. }),
                        Delta::Signature { signature: added },
                    ) => signature.push_str(&added),
                    _ => {}
                }
            }
            Event::MessageStart { message } => {
                if let Some(model) = message.model {
                    self.model = Some(model);
                }
                if let Some(usage) = &message.usage {
                    self.counts.take(usage);
                }
            }
            Event::MessageDelta { delta, usage } => {
                if let Some(reason) = delta.stop_reason {
                    self.stop_reason = StopReason::from_anthropic(&reason);
                }
                if let Some(usage) = &usage {
                    self.counts.take(usage);
                }
            }
            Event::MessageStop => return Ok(Flow::Done),
            Event::Error { error } => {
                let kind = error.and_then(|e| e.kind).unwrap_or(Cow::Borrowed("error"));
                return Err(unavailable(format!("anthropic stream error {kind}")));
            }
            Event::Other => {}
        }
        Ok(Flow::Continue)
    }

    fn finish(self) -> ModelTurn {
        let mut turn = ModelTurn {
            stop_reason: self.stop_reason,
            usage: self.counts.usage(),
            model: self.model,
            ..ModelTurn::default()
        };
        let mut raw = Vec::with_capacity(self.blocks.len());
        for block in self.blocks.into_values() {
            match &block {
                Block::Text(text) => turn.text.push_str(text),
                Block::ToolUse { id, name, json } => turn.tool_calls.push(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: tool_input(json),
                }),
                Block::Thinking { .. } | Block::Other(_) => {}
            }
            raw.push(block.into_raw());
        }
        turn.raw = Some(Value::Array(raw));
        turn
    }
}

#[cfg(test)]
impl Assembler {
    fn take_json(&mut self, event: &Value) -> Result<Flow, TelmoniError> {
        self.take(&SseEvent {
            event: None,
            data: event.to_string(),
        })
    }
}

#[async_trait]
impl Model for Anthropic {
    async fn stream(
        &self,
        system: &str,
        transcript: &[Message],
        tools: &[ToolSpec],
        on_text: TextSink<'_>,
    ) -> Result<ModelTurn, TelmoniError> {
        let body = json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "system": system,
            "messages": messages(transcript),
            "tools": tools.iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters,
            })).collect::<Vec<_>>(),
            "stream": true,
        });
        let mut request = self
            .http
            .post(format!("{}/v1/messages", self.config.url))
            .header("anthropic-version", API_VERSION)
            .json(&body);
        if let Some(key) = &self.config.api_key {
            request = request.header("x-api-key", key.expose());
        }
        stream_turn::<Assembler>(request, on_text).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(events: &[Value]) -> ModelTurn {
        let mut assembler = Assembler::default();
        for event in events {
            assembler.take_json(event).unwrap();
        }
        assembler.finish()
    }

    #[test]
    fn the_cache_is_counted_among_the_input_and_the_last_output_total_kept() {
        let turn = feed(&[
            json!({"type":"message_start","message":{"model":"claude-x","usage":{"input_tokens":20,"cache_read_input_tokens":300,"cache_creation_input_tokens":50,"output_tokens":1}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}),
        ]);
        assert_eq!(turn.model.as_deref(), Some("claude-x"));
        assert_eq!(
            turn.usage,
            Some(TokenUsage {
                input_tokens: 370,
                output_tokens: 12,
                cache_read_tokens: 300,
                cache_write_tokens: 50,
                reasoning_tokens: 0,
            })
        );
    }

    #[test]
    fn a_stream_that_counts_nothing_has_no_usage() {
        let turn = feed(&[
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Hi"}}),
        ]);
        assert_eq!(turn.usage, None);
    }

    #[test]
    fn a_tool_call_is_rebuilt_from_its_json_deltas() {
        let turn = feed(&[
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Checking."}}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tu_1","name":"search","input":{}}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"query\": \"sla"}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"ck\"}"}}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
        ]);
        assert_eq!(turn.text, "Checking.");
        assert_eq!(turn.stop_reason, StopReason::ToolUse);
        assert_eq!(
            turn.tool_calls,
            vec![ToolCall {
                id: "tu_1".into(),
                name: "search".into(),
                input: json!({"query": "slack"}),
            }]
        );
        let raw = turn.raw.unwrap();
        assert_eq!(
            raw.pointer("/0"),
            Some(&json!({"type":"thinking","thinking":"hm","signature":"sig"}))
        );
        assert_eq!(raw.pointer("/2/input/query"), Some(&json!("slack")));
    }

    #[test]
    fn broken_tool_json_reaches_the_tool_as_null() {
        let turn = feed(&[
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"search"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":"}}),
        ]);
        assert_eq!(
            turn.tool_calls.first().map(|c| &c.input),
            Some(&Value::Null)
        );
    }

    #[test]
    fn a_stream_error_is_a_model_failure() {
        let mut assembler = Assembler::default();
        let err = assembler
            .take_json(&json!({"type":"error","error":{"type":"overloaded_error","message":"x"}}))
            .unwrap_err();
        assert!(matches!(err, TelmoniError::AgentModelUnavailable { .. }));
    }
}
