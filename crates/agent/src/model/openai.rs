//! Chat Completions, streaming, with tool calls — the protocol OpenAI,
//! Gemini's compatible endpoint, Ollama, vLLM, OpenRouter and LM Studio all
//! serve. A tool call arrives as fragments keyed by its index: the id and
//! the name once, the arguments a piece at a time.
//!
//! Gemini's endpoint departs from that in two ways this file absorbs: it
//! sends each call whole with no `index`, and a Gemini 3 model attaches a
//! thought signature (`extra_content`) to its calls that must come back on
//! the next request, or the second round of a tool loop is refused.

use std::borrow::Cow;
use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use telmoni_shared::TelmoniError;

use super::{
    Assemble, Endpoint, Flow, Message, Model, ModelTurn, SseEvent, StopReason, TextSink,
    TokenUsage, ToolCall, ToolSpec, stream_turn, tool_input, unavailable, with_key,
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

/// One message of the transcript in Chat Completions' shape.
#[derive(Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub(crate) enum ChatMessage<'a> {
    System {
        content: &'a str,
    },
    User {
        content: &'a str,
    },
    Assistant {
        content: Option<&'a str>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ChatToolCall<'a>>,
    },
    Tool {
        tool_call_id: &'a str,
        content: Cow<'a, str>,
    },
}

/// One call of an assistant message, as the request carries it.
#[derive(Serialize)]
pub(crate) struct ChatToolCall<'a> {
    id: &'a str,
    r#type: &'static str,
    function: ChatFunction<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra_content: Option<&'a Value>,
}

#[derive(Serialize)]
struct ChatFunction<'a> {
    name: &'a str,
    arguments: String,
}

/// The system prompt and the transcript in Chat Completions' shape.
pub(crate) fn messages<'a>(system: &'a str, transcript: &'a [Message]) -> Vec<ChatMessage<'a>> {
    let mut out = vec![ChatMessage::System { content: system }];
    for message in transcript {
        match message {
            Message::User(text) => out.push(ChatMessage::User { content: text }),
            Message::Assistant {
                text,
                tool_calls,
                raw,
            } => {
                let extras = raw.as_ref().and_then(|r| r.get("extra_content"));
                out.push(ChatMessage::Assistant {
                    content: (!text.is_empty()).then_some(text.as_str()),
                    tool_calls: tool_calls
                        .iter()
                        .map(|c| ChatToolCall {
                            id: &c.id,
                            r#type: "function",
                            function: ChatFunction {
                                name: &c.name,
                                arguments: if c.input.is_null() {
                                    "{}".to_owned()
                                } else {
                                    c.input.to_string()
                                },
                            },
                            extra_content: extras.and_then(|e| e.get(&c.id)),
                        })
                        .collect(),
                });
            }
            Message::ToolResults(results) => {
                for r in results {
                    out.push(ChatMessage::Tool {
                        tool_call_id: &r.id,
                        content: if r.is_error {
                            Cow::Owned(format!("error: {}", r.content))
                        } else {
                            Cow::Borrowed(&r.content)
                        },
                    });
                }
            }
        }
    }
    out
}

/// One streamed chunk, framed as the server sends it: Google sends an error
/// mid-stream as a one-element array.
#[derive(Deserialize)]
#[serde(untagged)]
enum Framed<'a> {
    One(#[serde(borrow)] Chunk<'a>),
    Many(#[serde(borrow)] Vec<Chunk<'a>>),
}

#[derive(Deserialize, Default)]
struct Chunk<'a> {
    #[serde(default)]
    error: Option<ChunkError>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    model: Option<String>,
    #[serde(borrow, default)]
    choices: Vec<Choice<'a>>,
}

/// An error's name, in whichever field a server puts it.
#[derive(Deserialize)]
struct ChunkError {
    #[serde(default, rename = "type")]
    kind: Option<Value>,
    #[serde(default)]
    status: Option<Value>,
    #[serde(default)]
    code: Option<Value>,
}

/// Chat Completions' `usage`, each count where it is given.
#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    prompt_tokens_details: Option<PromptDetails>,
    #[serde(default)]
    completion_tokens_details: Option<CompletionDetails>,
}

#[derive(Deserialize)]
struct PromptDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct CompletionDetails {
    #[serde(default)]
    reasoning_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct Choice<'a> {
    #[serde(borrow, default)]
    finish_reason: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    delta: Delta<'a>,
}

#[derive(Deserialize, Default)]
struct Delta<'a> {
    #[serde(borrow, default)]
    content: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    tool_calls: Vec<ToolCallDelta<'a>>,
}

/// A tool call's fragment: the id and the name once, the arguments a piece
/// at a time.
#[derive(Deserialize)]
struct ToolCallDelta<'a> {
    #[serde(default)]
    index: Option<u64>,
    #[serde(borrow, default)]
    id: Option<Cow<'a, str>>,
    #[serde(default)]
    extra_content: Option<Value>,
    #[serde(borrow, default)]
    function: Function<'a>,
}

#[derive(Deserialize, Default)]
struct Function<'a> {
    #[serde(borrow, default)]
    name: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    arguments: Option<Cow<'a, str>>,
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
    usage: Option<TokenUsage>,
    model: Option<String>,
}

/// Chat Completions' `usage`. OpenAI sends it once, in a last chunk with no
/// choices; Gemini's endpoint repeats the totals so far on its chunks, so
/// each one read replaces the last rather than adding to it, which would
/// count a reply's tokens once per chunk.
fn usage(usage: &Usage) -> Option<TokenUsage> {
    Some(TokenUsage {
        input_tokens: usage.prompt_tokens?,
        output_tokens: usage.completion_tokens.unwrap_or(0),
        cache_read_tokens: usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens)
            .unwrap_or(0),
        cache_write_tokens: 0,
        reasoning_tokens: usage
            .completion_tokens_details
            .as_ref()
            .and_then(|d| d.reasoning_tokens)
            .unwrap_or(0),
    })
}

impl Assembler {
    /// The server said `[DONE]`: a stream that named no `finish_reason`
    /// still ended where its server meant it to.
    fn done(&mut self) {
        if self.finish_reason == StopReason::Unfinished {
            self.finish_reason = StopReason::Finished;
        }
    }
}

#[cfg(test)]
impl Assembler {
    fn take_json(&mut self, chunk: &Value) -> Result<Flow, TelmoniError> {
        self.take(&SseEvent {
            event: None,
            data: chunk.to_string(),
        })
    }
}

impl Assemble for Assembler {
    const ENDPOINT: Endpoint = Endpoint::OpenAiCompatible;

    fn take(&mut self, event: &SseEvent) -> Result<Flow, TelmoniError> {
        if event.data.trim() == "[DONE]" {
            self.done();
            return Ok(Flow::Done);
        }
        let chunk = match serde_json::from_str::<Framed<'_>>(&event.data) {
            Ok(Framed::One(chunk)) => chunk,
            Ok(Framed::Many(chunks)) => chunks.into_iter().next().unwrap_or_default(),
            Err(_) => return Ok(Flow::Continue),
        };
        if let Some(error) = chunk.error {
            let kind = [error.kind, error.status, error.code]
                .into_iter()
                .flatten()
                .next();
            return Err(unavailable(format!(
                "openai-compatible stream error {}",
                kind.as_ref().and_then(Value::as_str).unwrap_or("error")
            )));
        }
        if let Some(counted) = chunk.usage.as_ref().and_then(usage) {
            self.usage = Some(counted);
        }
        if let Some(model) = chunk.model {
            self.model = Some(model);
        }
        let Some(choice) = chunk.choices.into_iter().next() else {
            return Ok(Flow::Continue);
        };
        if let Some(reason) = &choice.finish_reason {
            self.finish_reason = StopReason::from_openai(reason);
        }
        for call in choice.delta.tool_calls {
            let id = call.id.as_deref();
            let index = match call.index {
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
            if let Some(extra) = call.extra_content {
                partial.extra_content = Some(extra);
            }
            // Some servers repeat the whole name on every fragment; appending
            // that made `searchsearch`, a tool that does not exist.
            if let Some(name) = call.function.name.as_deref()
                && partial.name != name
            {
                partial.name.push_str(name);
            }
            if let Some(arguments) = call.function.arguments.as_deref() {
                partial.arguments.push_str(arguments);
            }
        }
        match choice.delta.content {
            Some(text) if !text.is_empty() => {
                self.text.push_str(&text);
                Ok(Flow::Text(text.into_owned()))
            }
            _ => Ok(Flow::Continue),
        }
    }

    fn finish(self) -> ModelTurn {
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
                    input: tool_input(&call.arguments),
                }
            })
            .collect();
        ModelTurn {
            text: self.text,
            tool_calls,
            raw: (!extras.is_empty()).then(|| json!({ "extra_content": extras })),
            stop_reason: self.finish_reason,
            usage: self.usage,
            model: self.model,
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
            // OpenAI counts a streamed reply's tokens only when asked; the
            // servers that count anyway take the field as given.
            "stream_options": { "include_usage": true },
        });
        let request = with_key(
            self.http
                .post(format!("{}/chat/completions", self.config.url))
                .json(&body),
            self.config.api_key.as_ref(),
        );
        stream_turn::<Assembler>(request, on_text).await
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
            assembler.take_json(&chunk).unwrap();
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
            assembler.take_json(&chunk).unwrap();
        }
        let turn = assembler.finish();
        assert_eq!(turn.tool_calls.len(), 2);
        assert_eq!(turn.tool_calls[0].name, "list_connectors");
        assert_eq!(turn.tool_calls[1].id, "g2");
        assert_eq!(turn.tool_calls[1].input, json!({"query": "x"}));
    }

    #[test]
    fn the_usage_chunk_after_the_last_choice_is_read() {
        let mut assembler = Assembler::default();
        for chunk in [
            json!({"model":"gpt-x","choices":[{"delta":{"content":"Hi"}}],"usage":null}),
            json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":null}),
            json!({"choices":[],"usage":{"prompt_tokens":120,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":100},"completion_tokens_details":{"reasoning_tokens":3}}}),
        ] {
            assembler.take_json(&chunk).unwrap();
        }
        let turn = assembler.finish();
        assert_eq!(turn.model.as_deref(), Some("gpt-x"));
        assert_eq!(
            turn.usage,
            Some(TokenUsage {
                input_tokens: 120,
                output_tokens: 7,
                cache_read_tokens: 100,
                cache_write_tokens: 0,
                reasoning_tokens: 3,
            })
        );
    }

    #[test]
    fn totals_repeated_on_every_chunk_are_not_added_up() {
        let mut assembler = Assembler::default();
        for (n, text) in ["a", "b", "c"].into_iter().enumerate() {
            assembler
                .take_json(&json!({"choices":[{"delta":{"content":text}}],"usage":{"prompt_tokens":50,"completion_tokens":n + 1}}))
                .unwrap();
        }
        let usage = assembler.finish().usage.unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (50, 3));
    }

    #[test]
    fn a_google_error_array_mid_stream_is_an_error() {
        let chunk = json!([{"error": {"code": 503, "status": "UNAVAILABLE"}}]);
        assert!(Assembler::default().take_json(&chunk).is_err());
    }

    #[test]
    fn a_thought_signature_goes_back_on_its_call() {
        let signature = json!({"google": {"thought_signature": "sig"}});
        let mut assembler = Assembler::default();
        assembler
            .take_json(&json!({"choices":[{"delta":{"tool_calls":[{"id":"g1","extra_content":signature,"function":{"name":"search","arguments":"{}"}}]}}]}))
            .unwrap();
        let out = serde_json::to_value(messages(
            "sys",
            &[Message::User("q".into()), assembler.finish().into_message()],
        ))
        .unwrap();
        assert_eq!(out[2]["tool_calls"][0]["extra_content"], signature);
    }

    #[test]
    fn the_transcript_puts_each_result_on_its_own_tool_message() {
        let out = serde_json::to_value(messages(
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
        ))
        .unwrap();
        assert_eq!(out.as_array().map(Vec::len), Some(4));
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[2]["content"], Value::Null);
        assert_eq!(
            out[2]["tool_calls"][0]["function"]["arguments"],
            "{\"query\":\"x\"}"
        );
        assert_eq!(out[3]["tool_call_id"], "c1");
    }
}
