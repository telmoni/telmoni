//! A question as the SDK reports it: a run of the operator's own project,
//! sent with `record_run` the way any agent reports a run that has ended, so
//! it meets every check a customer's export does — the key, the rate limit,
//! the claim on its digest, the organization's grants — and ingest hashes the
//! asking organization as it hashes every user id. The SDK's own thread sends
//! it, and a flush on a blocking task sends it now; what the door refuses the
//! SDK logs once per kind, and the question is lost to the record, not to the
//! asker. A record's span ids derive from the question's id, so one sent
//! twice is the same bytes, which ingest answers once.
//!
//! The SDK is configured as a customer configures it, by `TELMONI_API_KEY`
//! and `TELMONI_ENDPOINT` ([`crate::config::Config::telmoni`]); the record
//! holds no content, and the SDK sends none.

use std::collections::BTreeMap;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use telmoni::{ModelCallRecord, RunRecord, Telmoni, ToolCallRecord, Usage};

use crate::observe::{Generation, Step, ToolUse, TurnEnding, TurnRecord};

/// The agent every question is a run of: the console agent's own name.
pub(crate) const AGENT: &str = "console-agent";

/// How a tool call that answered the model with an error, or was given up,
/// is classed: the agent records that it failed and not how.
pub(crate) const TOOL_ERROR_TYPE: &str = "ToolError";

/// How a question that left the turn by a path that named no ending is
/// classed.
pub(crate) const UNFINISHED_ERROR_TYPE: &str = "Unfinished";

/// The tag that says why an answer ended early, when it did.
pub(crate) const STOPPED_SHORT_TAG: &str = "stopped_short";

/// The tag that says what the question was asked about: `project` or
/// `organization`.
pub(crate) const ASKED_ABOUT_TAG: &str = "asked_about";

/// Report one question: queued here, which costs no wait, and sent from a
/// blocking task, since the flush waits on the SDK's thread. Without a
/// runtime to spawn on, the SDK's own batch sends it within its delay.
pub(crate) fn send(telmoni: &Telmoni, record: &TurnRecord) {
    let run_id = record.trace_id;
    if let Err(error) = telmoni.record_run(run_record(record)) {
        tracing::warn!(
            run_id = %run_id,
            error = %error,
            "the SDK refused a question's record"
        );
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let telmoni = telmoni.clone();
    handle.spawn_blocking(move || {
        if !telmoni.flush() {
            tracing::warn!(
                run_id = %run_id,
                "a question's record was not sent within the wait"
            );
        }
    });
}

/// A question as the SDK records a run: the question itself, each model call
/// and tool call under it in the order they happened, the asking
/// organization's id as its user, and the agent's conversation as its
/// session.
pub(crate) fn run_record(record: &TurnRecord) -> RunRecord {
    let (error_type, cancelled) = match &record.ending {
        TurnEnding::Answered { .. } => (None, false),
        TurnEnding::Failed { problem_type } => (Some(problem_type.clone()), false),
        TurnEnding::Unfinished => (Some(UNFINISHED_ERROR_TYPE.to_owned()), false),
        TurnEnding::Cancelled => (None, true),
    };
    let mut tags = BTreeMap::new();
    if let TurnEnding::Answered {
        stopped_short: Some(why),
    } = &record.ending
    {
        tags.insert(STOPPED_SHORT_TAG.to_owned(), why.as_str().to_owned());
    }
    tags.insert(
        ASKED_ABOUT_TAG.to_owned(),
        if record.project_id.is_some() {
            "project".to_owned()
        } else {
            "organization".to_owned()
        },
    );
    let (start, end) = span(record.started_at, record.ended_at);
    RunRecord {
        user_id: Some(record.organization_id.to_string()),
        session_id: Some(record.conversation_id.to_string()),
        tags,
        inside: record
            .steps
            .iter()
            .map(|step| match step {
                Step::Generation(generation) => model_call(generation).into(),
                Step::Tool(call) => tool_call(call).into(),
            })
            .collect(),
        error_type,
        cancelled,
        ..RunRecord::new(record.trace_id.to_string(), AGENT, start, end)
    }
}

/// The model of a call whose stream and configuration both named none: the
/// SDK refuses a whole record for a model named nothing.
const UNNAMED_MODEL: &str = "unknown";

/// One model call as the SDK records it: its model and provider, its tokens
/// as the conventions count them, and its error's type when it failed.
fn model_call(generation: &Generation) -> ModelCallRecord {
    let (start, end) = span(generation.started_at, generation.ended_at);
    let model = if generation.model.is_empty() {
        UNNAMED_MODEL.to_owned()
    } else {
        generation.model.clone()
    };
    ModelCallRecord {
        provider: Some(generation.provider.to_owned()),
        usage: generation.usage.map(|usage| Usage {
            input_tokens: Some(count(usage.input_tokens)),
            output_tokens: Some(count(usage.output_tokens)),
            cache_read_input_tokens: Some(count(usage.cache_read_tokens)),
            cache_write_input_tokens: Some(count(usage.cache_write_tokens)),
            // No adapter reads a cost, so ingest prices the tokens.
            cost: None,
        }),
        error_type: generation.error.clone(),
        ..ModelCallRecord::new(model, start, end)
    }
}

/// One tool call as the SDK records it: its name, and whether it failed.
fn tool_call(call: &ToolUse) -> ToolCallRecord {
    let (start, end) = span(call.started_at, call.ended_at);
    ToolCallRecord {
        error_type: call.is_error.then(|| TOOL_ERROR_TYPE.to_owned()),
        ..ToolCallRecord::new(call.name, start, end)
    }
}

/// A span's times as the SDK takes them, its end never before its start: the
/// SDK refuses a whole record for one such span, and a clock that stepped
/// back during a call is no reason to lose the question.
fn span(start: DateTime<Utc>, end: DateTime<Utc>) -> (SystemTime, SystemTime) {
    let start = SystemTime::from(start);
    (start, SystemTime::from(end).max(start))
}

/// A token count as the wire carries it.
fn count(tokens: u64) -> i64 {
    i64::try_from(tokens).unwrap_or(i64::MAX)
}

/// The SDK on a transport that keeps every export, for the tests here and in
/// `observe`.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use serde_json::Value;
    use telmoni::transport::{Answer, Transport, TransportError};
    use telmoni::{Config, Hooks, Telmoni};

    /// One export as the SDK handed it to the transport: its URL, its
    /// headers and its body.
    pub(crate) type Export = (String, Vec<(String, String)>, Vec<u8>);

    /// Keeps every export the SDK sends, answering each as taken.
    #[derive(Clone, Default)]
    pub(crate) struct Capture(Arc<Mutex<Vec<Export>>>);

    impl Capture {
        pub(crate) fn exports(&self) -> Vec<Export> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        /// The first export, once it has arrived.
        pub(crate) async fn first(&self) -> Export {
            for _ in 0..250 {
                if let Some(export) = self.exports().into_iter().next() {
                    return export;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("no export arrived within five seconds");
        }
    }

    impl Transport for Capture {
        fn send(
            &self,
            url: &str,
            headers: &[(String, String)],
            body: &[u8],
        ) -> Result<Answer, TransportError> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).push((
                url.to_owned(),
                headers.to_vec(),
                body.to_vec(),
            ));
            Ok(Answer::new(200, Vec::new(), b"{}".to_vec()))
        }
    }

    /// A client on `capture`, keyed and pointed at this machine.
    pub(crate) fn client(capture: &Capture) -> Telmoni {
        Telmoni::with_hooks(
            Config::new("http://127.0.0.1:1", None, Some("tm_test_key".into())),
            Hooks {
                transport: Box::new(capture.clone()),
                sleep: Box::new(|_| {}),
                warn: Box::new(|_, _| {}),
                ..Hooks::default()
            },
        )
    }

    /// The spans of an export, in the order they were sent.
    pub(crate) fn spans(body: &[u8]) -> Vec<Value> {
        let decoded: Value = serde_json::from_slice(body).unwrap_or_default();
        decoded["resourceSpans"][0]["scopeSpans"][0]["spans"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// An attribute's value on a span, as OTLP's JSON spells it: the one
    /// field of its `value`.
    pub(crate) fn attribute(span: &Value, key: &str) -> Option<Value> {
        span["attributes"]
            .as_array()?
            .iter()
            .find(|attribute| attribute["key"] == key)
            .and_then(|attribute| attribute["value"].as_object())
            .and_then(|value| value.values().next().cloned())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::SubsecRound as _;
    use serde_json::Value;
    use telmoni::RecordItem;
    use telmoni_shared::{OrganizationId, ProjectId};
    use uuid::Uuid;

    use super::testing::{Capture, attribute, client, spans};
    use super::*;
    use crate::model::TokenUsage;
    use crate::turn::StoppedShort;

    fn record(ending: TurnEnding) -> TurnRecord {
        let started = Utc::now().trunc_subsecs(6) - chrono::Duration::seconds(10);
        TurnRecord {
            trace_id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            organization_id: OrganizationId::new(),
            project_id: None,
            started_at: started,
            ended_at: started + chrono::Duration::seconds(8),
            ending,
            steps: vec![
                Step::Generation(Generation {
                    started_at: started,
                    ended_at: started + chrono::Duration::seconds(3),
                    provider: "anthropic",
                    model: "claude-sonnet-5-5".into(),
                    usage: Some(TokenUsage {
                        input_tokens: 2_000,
                        output_tokens: 120,
                        cache_read_tokens: 1_500,
                        cache_write_tokens: 0,
                        reasoning_tokens: 40,
                    }),
                    error: None,
                }),
                Step::Tool(ToolUse {
                    started_at: started + chrono::Duration::seconds(3),
                    ended_at: started + chrono::Duration::seconds(4),
                    name: "list_members",
                    is_error: true,
                }),
                Step::Generation(Generation {
                    started_at: started + chrono::Duration::seconds(4),
                    ended_at: started + chrono::Duration::seconds(8),
                    provider: "anthropic",
                    model: "claude-sonnet-5-5".into(),
                    usage: None,
                    error: Some("/errors/agent/model-unavailable".into()),
                }),
            ],
        }
    }

    fn model_call_of(item: &RecordItem) -> &ModelCallRecord {
        match item {
            RecordItem::ModelCall(call) => call,
            other => panic!("not a model call: {other:?}"),
        }
    }

    fn tool_call_of(item: &RecordItem) -> &ToolCallRecord {
        match item {
            RecordItem::ToolCall(call) => call,
            other => panic!("not a tool call: {other:?}"),
        }
    }

    /// The question is the run; each call a record under it, in order; the
    /// asking organization its user and the conversation its session.
    #[test]
    fn a_question_is_a_run_and_its_calls_are_records_under_it() {
        let turn = record(TurnEnding::Answered {
            stopped_short: Some(StoppedShort::Capped),
        });
        let run = run_record(&turn);
        assert_eq!(run.run_id, turn.trace_id.to_string());
        assert_eq!(run.agent, AGENT);
        assert_eq!(
            run.user_id.as_deref(),
            Some(turn.organization_id.as_str()),
            "the asking organization is the user, never the person"
        );
        assert_eq!(run.session_id, Some(turn.conversation_id.to_string()));
        assert_eq!(
            run.tags,
            BTreeMap::from([
                (ASKED_ABOUT_TAG.to_owned(), "organization".to_owned()),
                (STOPPED_SHORT_TAG.to_owned(), "capped".to_owned()),
            ])
        );
        assert_eq!(run.start, SystemTime::from(turn.started_at));
        assert_eq!(run.end, SystemTime::from(turn.ended_at));
        assert_eq!(run.error_type, None);
        assert!(!run.cancelled);
        assert_eq!(run.inside.len(), 3);

        let first = model_call_of(&run.inside[0]);
        assert_eq!(first.model, "claude-sonnet-5-5");
        assert_eq!(first.provider.as_deref(), Some("anthropic"));
        assert_eq!(first.operation, None, "a chat, as the SDK names one");
        assert_eq!(
            first.usage,
            Some(Usage {
                input_tokens: Some(2_000),
                output_tokens: Some(120),
                cache_read_input_tokens: Some(1_500),
                cache_write_input_tokens: Some(0),
                cost: None,
            })
        );
        assert_eq!(first.error_type, None);
        assert_eq!(first.start, SystemTime::from(turn.started_at));

        let tool = tool_call_of(&run.inside[1]);
        assert_eq!(tool.name, "list_members");
        assert_eq!(tool.error_type.as_deref(), Some(TOOL_ERROR_TYPE));

        let failed = model_call_of(&run.inside[2]);
        assert_eq!(failed.usage, None, "no usage, no tokens");
        assert_eq!(
            failed.error_type.as_deref(),
            Some("/errors/agent/model-unavailable")
        );
    }

    #[test]
    fn each_ending_is_an_error_type_or_a_cancel() {
        let ending = |ending| {
            let run = run_record(&record(ending));
            (run.error_type, run.cancelled, run.tags)
        };
        let (none, cancelled, tags) = ending(TurnEnding::Answered {
            stopped_short: None,
        });
        assert_eq!((none, cancelled), (None, false));
        assert_eq!(
            tags,
            BTreeMap::from([(ASKED_ABOUT_TAG.to_owned(), "organization".to_owned())])
        );
        let (problem, cancelled, _) = ending(TurnEnding::Failed {
            problem_type: "/errors/agent/budget".into(),
        });
        assert_eq!(
            (problem.as_deref(), cancelled),
            (Some("/errors/agent/budget"), false)
        );
        let (unfinished, cancelled, _) = ending(TurnEnding::Unfinished);
        assert_eq!(
            (unfinished.as_deref(), cancelled),
            (Some(UNFINISHED_ERROR_TYPE), false)
        );
        let (none, cancelled, _) = ending(TurnEnding::Cancelled);
        assert_eq!((none, cancelled), (None, true));

        let mut about_project = record(TurnEnding::Cancelled);
        about_project.project_id = Some(ProjectId::new());
        assert_eq!(
            run_record(&about_project)
                .tags
                .get(ASKED_ABOUT_TAG)
                .map(String::as_str),
            Some("project")
        );
    }

    /// The SDK refuses a whole record for a span that ends before it starts;
    /// one such call is sent as instant instead, so the question is kept.
    #[test]
    fn a_call_that_ends_before_it_starts_is_sent_as_instant_not_refused() {
        let mut turn = record(TurnEnding::Answered {
            stopped_short: None,
        });
        if let Step::Tool(call) = &mut turn.steps[1] {
            call.ended_at = call.started_at - chrono::Duration::seconds(1);
        }
        let run = run_record(&turn);
        let tool = tool_call_of(&run.inside[1]);
        assert_eq!(tool.end, tool.start);
    }

    /// A call whose model nothing named is sent as `unknown`, so the question is kept.
    #[test]
    fn a_call_whose_model_was_named_nowhere_is_sent_as_unknown_not_refused() {
        let mut turn = record(TurnEnding::Answered {
            stopped_short: None,
        });
        if let Step::Generation(generation) = &mut turn.steps[0] {
            generation.model.clear();
        }
        let run = run_record(&turn);
        assert_eq!(model_call_of(&run.inside[0]).model, UNNAMED_MODEL);
    }

    /// The export is OTLP's JSON to the door under the key, its spans named
    /// and tagged as the contract reads them, and no content in it.
    #[tokio::test]
    async fn the_export_leaves_through_the_door_under_the_key_with_the_contracts_names() {
        let capture = Capture::default();
        let telmoni = client(&capture);
        let turn = record(TurnEnding::Failed {
            problem_type: "/errors/agent/model-unavailable".into(),
        });
        send(&telmoni, &turn);

        let (url, headers, body) = capture.first().await;
        assert_eq!(url, "http://127.0.0.1:1/otlp/v1/traces");
        let header = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(header("Authorization"), Some("Bearer tm_test_key"));
        assert_eq!(header("Content-Type"), Some("application/json"));

        let text = String::from_utf8(body.clone()).unwrap_or_default();
        let spans = spans(&body);
        let names: Vec<&str> = spans
            .iter()
            .map(|span| span["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            [
                "chat claude-sonnet-5-5",
                "execute_tool list_members",
                "chat claude-sonnet-5-5",
                "invoke_agent console-agent",
            ],
            "the run's own span last, as a record is sent"
        );
        let run = &spans[3];
        assert_eq!(
            attribute(run, "telmoni.run_id"),
            Some(Value::from(turn.trace_id.to_string()))
        );
        assert_eq!(attribute(run, "telmoni.agent"), Some(Value::from(AGENT)));
        assert_eq!(
            attribute(run, "gen_ai.agent.name"),
            Some(Value::from(AGENT))
        );
        assert_eq!(
            attribute(run, "user.id"),
            Some(Value::from(turn.organization_id.as_str())),
            "the asking organization, for ingest to hash"
        );
        assert_eq!(
            attribute(run, "gen_ai.conversation.id"),
            Some(Value::from(turn.conversation_id.to_string()))
        );
        assert_eq!(
            attribute(run, "telmoni.tag.asked_about"),
            Some(Value::from("organization"))
        );
        assert_eq!(
            attribute(run, "error.type"),
            Some(Value::from("/errors/agent/model-unavailable"))
        );
        assert_eq!(run["status"]["code"], 2);
        for span in &spans {
            assert_eq!(span["traceId"], run["traceId"], "one trace, the question's");
        }

        let first = &spans[0];
        assert_eq!(
            attribute(first, "gen_ai.request.model"),
            Some(Value::from("claude-sonnet-5-5"))
        );
        assert_eq!(
            attribute(first, "gen_ai.provider.name"),
            Some(Value::from("anthropic"))
        );
        let tokens = attribute(first, "gen_ai.usage.input_tokens")
            .map(|value| value.to_string().trim_matches('"').to_owned());
        assert_eq!(tokens.as_deref(), Some("2000"));
        assert_eq!(
            attribute(&spans[1], "gen_ai.tool.name"),
            Some(Value::from("list_members"))
        );
        assert_eq!(
            attribute(&spans[1], "error.type"),
            Some(Value::from(TOOL_ERROR_TYPE))
        );
        assert_eq!(
            attribute(&spans[2], "error.type"),
            Some(Value::from("/errors/agent/model-unavailable"))
        );

        // ⚠ Ids, names of ours, counts and types: no content attribute.
        for content in [
            "gen_ai.input.messages",
            "gen_ai.output.messages",
            "gen_ai.system_instructions",
            "gen_ai.tool.call.arguments",
            "gen_ai.tool.call.result",
        ] {
            assert!(!text.contains(content), "{content} was sent");
        }
    }

    /// A record the SDK refuses — a question with no id — sends nothing, and
    /// the question is lost to the record alone.
    #[tokio::test]
    async fn a_record_the_sdk_refuses_sends_nothing() {
        let capture = Capture::default();
        let telmoni = client(&capture);
        let mut turn = record(TurnEnding::Answered {
            stopped_short: None,
        });
        turn.trace_id = Uuid::nil();
        send(&telmoni, &turn);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(capture.exports().is_empty());
    }
}
