//! The Messages API, streaming, with tool use.
//!
//! Each content block is rebuilt from its deltas and the whole turn is kept
//! as `raw`, so a thinking block goes back exactly as it came — a model that
//! thinks by default refuses a transcript whose thinking was altered.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{Value, json};
use telmoni_shared::TelmoniError;

use super::{
    Message, Model, ModelTurn, StopReason, TextSink, ToolCall, ToolSpec, read_events, send,
    unavailable,
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

/// The transcript in the Messages API's shape.
pub(crate) fn messages(transcript: &[Message]) -> Vec<Value> {
    transcript
        .iter()
        .map(|message| match message {
            Message::User(text) => json!({ "role": "user", "content": text }),
            Message::Assistant {
                raw: Some(raw), ..
            } => json!({ "role": "assistant", "content": raw }),
            Message::Assistant {
                text, tool_calls, ..
            } => {
                let mut content = Vec::new();
                if !text.is_empty() {
                    content.push(json!({ "type": "text", "text": text }));
                }
                for call in tool_calls {
                    content.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": if call.input.is_object() { call.input.clone() } else { json!({}) },
                    }));
                }
                json!({ "role": "assistant", "content": content })
            }
            Message::ToolResults(results) => json!({
                "role": "user",
                "content": results.iter().map(|r| json!({
                    "type": "tool_result",
                    "tool_use_id": r.id,
                    "content": r.content,
                    "is_error": r.is_error,
                })).collect::<Vec<_>>(),
            }),
        })
        .collect()
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
            Self::ToolUse { id, name, json } => json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": parse_input(&json).filter(Value::is_object).unwrap_or_else(|| json!({})),
            }),
            Self::Thinking {
                thinking,
                signature,
            } => json!({ "type": "thinking", "thinking": thinking, "signature": signature }),
            Self::Other(value) => value,
        }
    }
}

fn parse_input(json: &str) -> Option<Value> {
    if json.trim().is_empty() {
        return Some(json!({}));
    }
    serde_json::from_str(json).ok()
}

/// The streamed events of one turn, folded into its blocks.
#[derive(Debug, Default)]
pub(crate) struct Assembler {
    blocks: BTreeMap<u64, Block>,
    stop_reason: StopReason,
}

impl Assembler {
    /// Take one event; answer the text it added, if any.
    pub(crate) fn take(&mut self, event: &Value) -> Result<Option<String>, TelmoniError> {
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
        match event.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                if let Some(block) = event.get("content_block") {
                    let block = Block::start(block);
                    let initial = match &block {
                        Block::Text(text) if !text.is_empty() => Some(text.clone()),
                        _ => None,
                    };
                    self.blocks.insert(index, block);
                    return Ok(initial);
                }
            }
            Some("content_block_delta") => {
                let delta = event.get("delta").unwrap_or(&Value::Null);
                let piece = |key: &str| delta.get(key).and_then(Value::as_str).unwrap_or_default();
                match (
                    self.blocks.get_mut(&index),
                    delta.get("type").and_then(Value::as_str),
                ) {
                    (Some(Block::Text(text)), Some("text_delta")) => {
                        let added = piece("text");
                        text.push_str(added);
                        return Ok(Some(added.to_owned()));
                    }
                    (Some(Block::ToolUse { json, .. }), Some("input_json_delta")) => {
                        json.push_str(piece("partial_json"));
                    }
                    (Some(Block::Thinking { thinking, .. }), Some("thinking_delta")) => {
                        thinking.push_str(piece("thinking"));
                    }
                    (Some(Block::Thinking { signature, .. }), Some("signature_delta")) => {
                        signature.push_str(piece("signature"));
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(reason) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = StopReason::from_anthropic(reason);
                }
            }
            Some("error") => {
                let kind = event
                    .pointer("/error/type")
                    .and_then(Value::as_str)
                    .unwrap_or("error");
                return Err(unavailable(format!("anthropic stream error {kind}")));
            }
            _ => {}
        }
        Ok(None)
    }

    pub(crate) fn finish(self) -> ModelTurn {
        let mut turn = ModelTurn {
            stop_reason: self.stop_reason,
            ..ModelTurn::default()
        };
        let mut raw = Vec::with_capacity(self.blocks.len());
        for block in self.blocks.into_values() {
            match &block {
                Block::Text(text) => turn.text.push_str(text),
                Block::ToolUse { id, name, json } => turn.tool_calls.push(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: parse_input(json).unwrap_or(Value::Null),
                }),
                Block::Thinking { .. } | Block::Other(_) => {}
            }
            raw.push(block.into_raw());
        }
        turn.raw = Some(Value::Array(raw));
        turn
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
        let response = send("anthropic", request).await?;

        let mut assembler = Assembler::default();
        read_events("anthropic", response, |event| {
            let Ok(event) = serde_json::from_str::<Value>(&event.data) else {
                return Ok(true);
            };
            if let Some(text) = assembler.take(&event)? {
                on_text(&text);
            }
            Ok(event.get("type").and_then(Value::as_str) != Some("message_stop"))
        })
        .await?;
        Ok(assembler.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(events: &[Value]) -> ModelTurn {
        let mut assembler = Assembler::default();
        for event in events {
            assembler.take(event).unwrap();
        }
        assembler.finish()
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
            .take(&json!({"type":"error","error":{"type":"overloaded_error","message":"x"}}))
            .unwrap_err();
        assert!(matches!(err, TelmoniError::AgentModelUnavailable { .. }));
    }
}
