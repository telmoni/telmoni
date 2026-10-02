//! Chat Completions, streaming, with tool calls — the protocol OpenAI,
//! Gemini's compatible endpoint, Ollama, vLLM, OpenRouter and LM Studio all
//! serve. A tool call arrives as fragments keyed by its index: the id and
//! the name once, the arguments a piece at a time.
//!
//! Gemini's endpoint departs from that in two ways this file absorbs: it
//! sends each call whole with no `index`, and a Gemini 3 model attaches a
//! thought signature (`extra_content`) to its calls that must come back on
//! the next request, or the second round of a tool loop is refused.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{Value, json};
use telmoni_shared::TelmoniError;

use super::{
    Message, Model, ModelTurn, StopReason, TextSink, ToolCall, ToolSpec, read_events, send,
    unavailable,
};
use crate::config::ModelConfig;

pub struct OpenAi {
    http: reqwest::Client,
    config: ModelConfig,
}

impl OpenAi {
    #[must_use]
    pub const fn new(http: reqwest::Client, config: ModelConfig) -> Self {
        Self { http, config }
    }
}

/// The system prompt and the transcript in Chat Completions' shape.
pub(crate) fn messages(system: &str, transcript: &[Message]) -> Vec<Value> {
    let mut out = vec![json!({ "role": "system", "content": system })];
    for message in transcript {
        match message {
            Message::User(text) => out.push(json!({ "role": "user", "content": text })),
            Message::Assistant {
                text,
                tool_calls,
                raw,
            } => {
                let extras = raw.as_ref().and_then(|r| r.get("extra_content"));
                let content = if text.is_empty() {
                    Value::Null
                } else {
                    json!(text)
                };
                if tool_calls.is_empty() {
                    out.push(json!({ "role": "assistant", "content": content }));
                    continue;
                }
                let calls: Vec<Value> = tool_calls
                    .iter()
                    .map(|c| {
                        let mut call = json!({
                            "id": c.id,
                            "type": "function",
                            "function": {
                                "name": c.name,
                                "arguments": if c.input.is_null() {
                                    "{}".to_owned()
                                } else {
                                    c.input.to_string()
                                },
                            },
                        });
                        if let (Some(fields), Some(extra)) =
                            (call.as_object_mut(), extras.and_then(|e| e.get(&c.id)))
                        {
                            fields.insert("extra_content".to_owned(), extra.clone());
                        }
                        call
                    })
                    .collect();
                out.push(json!({ "role": "assistant", "content": content, "tool_calls": calls }));
            }
            Message::ToolResults(results) => {
                for r in results {
                    let content = if r.is_error {
                        format!("error: {}", r.content)
                    } else {
                        r.content.clone()
                    };
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": r.id,
                        "content": content,
                    }));
                }
            }
        }
    }
    out
}

#[derive(Debug, Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
    extra_content: Option<Value>,
}

/// The streamed chunks of one turn, folded together.
#[derive(Debug, Default)]
pub(crate) struct Assembler {
    text: String,
    calls: BTreeMap<u64, PartialCall>,
    finish_reason: StopReason,
}

impl Assembler {
    /// Take one chunk; answer the text it added, if any.
    pub(crate) fn take(&mut self, chunk: &Value) -> Result<Option<String>, TelmoniError> {
        // Google sends an error mid-stream as a one-element array.
        let chunk = chunk.as_array().and_then(|a| a.first()).unwrap_or(chunk);
        if let Some(kind) = chunk
            .pointer("/error/type")
            .or_else(|| chunk.pointer("/error/status"))
            .or_else(|| chunk.pointer("/error/code"))
        {
            return Err(unavailable(format!(
                "openai-compatible stream error {}",
                kind.as_str().unwrap_or("error")
            )));
        }
        let Some(choice) = chunk.pointer("/choices/0") else {
            return Ok(None);
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = StopReason::from_openai(reason);
        }
        let delta = choice.get("delta").unwrap_or(&Value::Null);
        for call in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = call.get("id").and_then(Value::as_str);
            let index = match call.get("index").and_then(Value::as_u64) {
                Some(index) => index,
                // No index: a new id is a new call, a fragment without one
                // continues the last.
                None => match self.calls.last_key_value() {
                    None => 0,
                    Some((&last, partial)) if id.is_some_and(|id| id != partial.id) => last + 1,
                    Some((&last, _)) => last,
                },
            };
            let partial = self.calls.entry(index).or_default();
            if let Some(id) = id {
                id.clone_into(&mut partial.id);
            }
            if let Some(extra) = call.get("extra_content") {
                partial.extra_content = Some(extra.clone());
            }
            // Some servers repeat the whole name on every fragment; appending
            // that made `searchsearch`, a tool that does not exist.
            if let Some(name) = call.pointer("/function/name").and_then(Value::as_str)
                && partial.name != name
            {
                partial.name.push_str(name);
            }
            if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
                partial.arguments.push_str(arguments);
            }
        }
        match delta.get("content").and_then(Value::as_str) {
            Some(text) if !text.is_empty() => {
                self.text.push_str(text);
                Ok(Some(text.to_owned()))
            }
            _ => Ok(None),
        }
    }

    /// The server said `[DONE]`: a stream that named no `finish_reason`
    /// still ended where its server meant it to.
    pub(crate) fn done(&mut self) {
        if self.finish_reason == StopReason::Unfinished {
            self.finish_reason = StopReason::Finished;
        }
    }

    pub(crate) fn finish(self) -> ModelTurn {
        let mut extras = serde_json::Map::new();
        let tool_calls = self
            .calls
            .into_iter()
            .map(|(index, call)| {
                // Some local servers send no id; the result must still name one.
                let id = if call.id.is_empty() {
                    format!("call_{index}")
                } else {
                    call.id
                };
                if let Some(extra) = call.extra_content {
                    extras.insert(id.clone(), extra);
                }
                ToolCall {
                    id,
                    name: call.name,
                    input: if call.arguments.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(&call.arguments).unwrap_or(Value::Null)
                    },
                }
            })
            .collect();
        ModelTurn {
            text: self.text,
            tool_calls,
            raw: (!extras.is_empty()).then(|| json!({ "extra_content": extras })),
            stop_reason: self.finish_reason,
        }
    }
}

#[async_trait]
impl Model for OpenAi {
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
            "messages": messages(system, transcript),
            "tools": tools.iter().map(|t| json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                },
            })).collect::<Vec<_>>(),
            "stream": true,
        });
        let mut request = self
            .http
            .post(format!("{}/chat/completions", self.config.url))
            .json(&body);
        if let Some(key) = &self.config.api_key {
            request = request.bearer_auth(key.expose());
        }
        let response = send("openai-compatible", request).await?;

        let mut assembler = Assembler::default();
        read_events("openai-compatible", response, |event| {
            if event.data.trim() == "[DONE]" {
                assembler.done();
                return Ok(false);
            }
            let Ok(chunk) = serde_json::from_str::<Value>(&event.data) else {
                return Ok(true);
            };
            if let Some(text) = assembler.take(&chunk)? {
                on_text(&text);
            }
            Ok(true)
        })
        .await?;
        Ok(assembler.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ToolResult;

    #[test]
    fn tool_call_fragments_are_joined_by_index() {
        let mut assembler = Assembler::default();
        for chunk in [
            json!({"choices":[{"delta":{"content":"Let me look."}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"list_connectors","arguments":""}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":1,"id":"c2","function":{"name":"search","arguments":"{\"que"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"ry\":\"x\"}"}}]}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
        ] {
            assembler.take(&chunk).unwrap();
        }
        let turn = assembler.finish();
        assert_eq!(turn.text, "Let me look.");
        assert_eq!(turn.stop_reason, StopReason::ToolUse);
        assert_eq!(turn.tool_calls.len(), 2);
        assert_eq!(turn.tool_calls[0].input, json!({}));
        assert_eq!(turn.tool_calls[1].input, json!({"query": "x"}));
    }

    #[test]
    fn calls_without_an_index_stay_apart() {
        let mut assembler = Assembler::default();
        for chunk in [
            json!({"choices":[{"delta":{"tool_calls":[{"id":"g1","function":{"name":"list_connectors","arguments":"{}"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"id":"g2","function":{"name":"search","arguments":"{\"query\":\"x\"}"}}]}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
        ] {
            assembler.take(&chunk).unwrap();
        }
        let turn = assembler.finish();
        assert_eq!(turn.tool_calls.len(), 2);
        assert_eq!(turn.tool_calls[0].name, "list_connectors");
        assert_eq!(turn.tool_calls[1].id, "g2");
        assert_eq!(turn.tool_calls[1].input, json!({"query": "x"}));
    }

    #[test]
    fn a_google_error_array_mid_stream_is_an_error() {
        let chunk = json!([{"error": {"code": 503, "status": "UNAVAILABLE"}}]);
        assert!(Assembler::default().take(&chunk).is_err());
    }

    #[test]
    fn a_thought_signature_goes_back_on_its_call() {
        let signature = json!({"google": {"thought_signature": "sig"}});
        let mut assembler = Assembler::default();
        assembler
            .take(&json!({"choices":[{"delta":{"tool_calls":[{"id":"g1","extra_content":signature,"function":{"name":"search","arguments":"{}"}}]}}]}))
            .unwrap();
        let out = messages(
            "sys",
            &[Message::User("q".into()), assembler.finish().into_message()],
        );
        assert_eq!(out[2]["tool_calls"][0]["extra_content"], signature);
    }

    #[test]
    fn the_transcript_puts_each_result_on_its_own_tool_message() {
        let out = messages(
            "sys",
            &[
                Message::User("q".into()),
                Message::Assistant {
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "c1".into(),
                        name: "search".into(),
                        input: json!({"query":"x"}),
                    }],
                    raw: None,
                },
                Message::ToolResults(vec![ToolResult {
                    id: "c1".into(),
                    content: "found".into(),
                    is_error: false,
                }]),
            ],
        );
        assert_eq!(out.len(), 4);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[2]["content"], Value::Null);
        assert_eq!(
            out[2]["tool_calls"][0]["function"]["arguments"],
            "{\"query\":\"x\"}"
        );
        assert_eq!(out[3]["tool_call_id"], "c1");
    }
}
