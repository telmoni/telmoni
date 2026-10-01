//! The tool loop: ask the model, run what it calls, hand the results back,
//! until it answers without calling anything, the round cap is reached, or
//! the turn's time runs out.

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use telmoni_shared::TelmoniError;
use tokio::time::Instant;

use crate::model::{
    Message, Model, StopReason, TextSink, ToolCall, ToolResult, ToolSpec, unavailable,
};

/// The most rounds of tool calls one question may take. A model that is
/// still calling tools after this many is looping, and the person gets what
/// it wrote so far with a note, rather than a bill for a hundred calls.
pub const MAX_TOOL_ROUNDS: usize = 8;

/// How long one turn may run, every model call and tool included.
///
/// ⚠ **Under the console's `AGENT_STREAM_TIMEOUT_MS` (120 s, in
/// `web/lib/server/entities/agent.ts`)**, which cuts the stream wherever it
/// is. Nine model calls of `AGENT_MODEL_TIMEOUT_SECS` each can take far
/// longer, and a turn the console cut off reached the person as "couldn't
/// reach the agent" halfway through a reply, with nothing saved. Ending it
/// here first leaves room to save what was written, cite it and say `done`.
pub const TURN_BUDGET: Duration = Duration::from_secs(100);

/// What the person is told when the turn ran out of time.
const TIMED_OUT_NOTE: &str = "\n\nI ran out of time before finishing. Ask again, or try a \
                              narrower question.";

/// What the person is told when the cap cut the turn short.
const CAPPED_NOTE: &str = "\n\nI stopped after looking things up 8 times without reaching an \
                           answer. Try a narrower question.";

/// What the person is told when the reply ran past `AGENT_MAX_TOKENS`.
const TRUNCATED_NOTE: &str = "\n\nThat reply ran out of room and was cut off. Ask for the rest, \
                              or a narrower question.";

/// Whoever runs the model's calls: the live tools, or a test's double.
#[async_trait]
pub trait Runner: Send {
    /// Run one call.
    async fn run(&mut self, call: &ToolCall) -> ToolResult;

    /// Whether the asker has gone, so the loop stops spending on them.
    fn cancelled(&self) -> bool {
        false
    }
}

/// How the loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The model answered without calling anything more.
    Answered,
    /// [`MAX_TOOL_ROUNDS`] ran out first.
    Capped,
    /// The answer ran past the token ceiling.
    Truncated,
    /// The deadline passed first.
    TimedOut,
    /// The asker left; nothing is saved or sent.
    Cancelled,
}

impl Ending {
    /// What the person is told after the text, when the ending needs saying.
    #[must_use]
    pub const fn note(self) -> Option<&'static str> {
        match self {
            Self::Capped => Some(CAPPED_NOTE),
            Self::Truncated => Some(TRUNCATED_NOTE),
            Self::TimedOut => Some(TIMED_OUT_NOTE),
            Self::Answered | Self::Cancelled => None,
        }
    }
}

/// What the loop came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Every word the model wrote across the rounds.
    pub text: String,
    /// The rounds whose calls were run.
    pub rounds: usize,
    pub ending: Ending,
}

/// Run the loop over `transcript`, which ends with the person's question,
/// until `deadline`.
///
/// The answer is what reached `on_text`, not what the calls returned, so a
/// call the deadline cuts short keeps the words the person already read.
pub async fn converse(
    model: &dyn Model,
    system: &str,
    transcript: &mut Vec<Message>,
    tools: &[ToolSpec],
    runner: &mut dyn Runner,
    on_text: TextSink<'_>,
    deadline: Instant,
) -> Result<Outcome, TelmoniError> {
    let written = Mutex::new(String::new());
    let sink = |delta: &str| {
        if let Ok(mut written) = written.lock() {
            written.push_str(delta);
        }
        on_text(delta);
    };
    let text = || written.lock().map(|w| w.clone()).unwrap_or_default();
    let mut round = 0;
    loop {
        let ended = |ending| Outcome {
            text: text(),
            rounds: round,
            ending,
        };
        if Instant::now() >= deadline {
            return Ok(ended(Ending::TimedOut));
        }
        let Ok(turn) =
            tokio::time::timeout_at(deadline, model.stream(system, transcript, tools, &sink)).await
        else {
            return Ok(ended(Ending::TimedOut));
        };
        let turn = turn?;
        if runner.cancelled() {
            return Ok(ended(Ending::Cancelled));
        }
        if turn.stop_reason == StopReason::Refused && turn.text.is_empty() {
            return Err(unavailable("the model declined the request"));
        }
        if turn.tool_calls.is_empty() {
            let ending = match turn.stop_reason {
                StopReason::MaxTokens => Ending::Truncated,
                // A refusal with text is the model saying why it would not;
                // that is its answer.
                StopReason::Finished
                | StopReason::ToolUse
                | StopReason::Refused
                | StopReason::Other => Ending::Answered,
            };
            return Ok(ended(ending));
        }
        if round == MAX_TOOL_ROUNDS {
            return Ok(ended(Ending::Capped));
        }
        let mut results = Vec::with_capacity(turn.tool_calls.len());
        for call in &turn.tool_calls {
            results.push(runner.run(call).await);
        }
        transcript.push(turn.into_message());
        transcript.push(Message::ToolResults(results));
        round += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;
    use crate::model::ModelTurn;

    /// A model that calls a tool on every call, and counts them.
    struct Looping(AtomicUsize);

    #[async_trait]
    impl Model for Looping {
        async fn stream(
            &self,
            _system: &str,
            _messages: &[Message],
            _tools: &[ToolSpec],
            on_text: TextSink<'_>,
        ) -> Result<ModelTurn, TelmoniError> {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            on_text(".");
            Ok(ModelTurn {
                text: ".".into(),
                tool_calls: vec![ToolCall {
                    id: format!("call_{n}"),
                    name: "search".into(),
                    input: json!({"query": "again"}),
                }],
                raw: None,
                stop_reason: StopReason::ToolUse,
            })
        }
    }

    /// A model that calls one tool, then answers from its result.
    struct OnceThenAnswer(AtomicUsize);

    #[async_trait]
    impl Model for OnceThenAnswer {
        async fn stream(
            &self,
            _system: &str,
            messages: &[Message],
            _tools: &[ToolSpec],
            on_text: TextSink<'_>,
        ) -> Result<ModelTurn, TelmoniError> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(ModelTurn {
                    tool_calls: vec![ToolCall {
                        id: "c".into(),
                        name: "list_connectors".into(),
                        input: json!({}),
                    }],
                    ..ModelTurn::default()
                });
            }
            let Some(Message::ToolResults(results)) = messages.last() else {
                panic!("the second call must see the tool's result last");
            };
            let text = format!("Found: {} [1]", results[0].content);
            on_text(&text);
            Ok(ModelTurn {
                text,
                ..ModelTurn::default()
            })
        }
    }

    /// A turn with all the time it needs.
    fn unhurried() -> Instant {
        Instant::now() + TURN_BUDGET
    }

    #[derive(Default)]
    struct Counting(usize);

    #[async_trait]
    impl Runner for Counting {
        async fn run(&mut self, call: &ToolCall) -> ToolResult {
            self.0 += 1;
            ToolResult {
                id: call.id.clone(),
                content: "slack, errored".into(),
                is_error: false,
            }
        }
    }

    #[tokio::test]
    async fn the_loop_stops_at_the_round_cap() {
        let model = Looping(AtomicUsize::new(0));
        let mut runner = Counting::default();
        let streamed = Mutex::new(String::new());
        let sink = |t: &str| streamed.lock().unwrap().push_str(t);
        let mut transcript = vec![Message::User("why?".into())];
        let outcome = converse(
            &model,
            "sys",
            &mut transcript,
            &[],
            &mut runner,
            &sink,
            unhurried(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.ending, Ending::Capped);
        assert_eq!(runner.0, MAX_TOOL_ROUNDS, "tools ran past the cap");
        assert_eq!(model.0.load(Ordering::SeqCst), MAX_TOOL_ROUNDS + 1);
        assert_eq!(outcome.text.len(), MAX_TOOL_ROUNDS + 1);
        assert_eq!(*streamed.lock().unwrap(), outcome.text);
    }

    #[tokio::test]
    async fn a_tool_result_reaches_the_next_call_and_the_answer_ends_the_loop() {
        let model = OnceThenAnswer(AtomicUsize::new(0));
        let mut runner = Counting::default();
        let mut transcript = vec![Message::User("connectors?".into())];
        let outcome = converse(
            &model,
            "sys",
            &mut transcript,
            &[],
            &mut runner,
            &|_| {},
            unhurried(),
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            Outcome {
                text: "Found: slack, errored [1]".into(),
                rounds: 1,
                ending: Ending::Answered,
            }
        );
        assert_eq!(transcript.len(), 3);
    }

    /// A model that answers once, stopping as it is told to.
    struct Stops(StopReason, &'static str);

    #[async_trait]
    impl Model for Stops {
        async fn stream(
            &self,
            _system: &str,
            _messages: &[Message],
            _tools: &[ToolSpec],
            on_text: TextSink<'_>,
        ) -> Result<ModelTurn, TelmoniError> {
            on_text(self.1);
            Ok(ModelTurn {
                text: self.1.into(),
                stop_reason: self.0,
                ..ModelTurn::default()
            })
        }
    }

    /// A model that writes a few words, then never finishes.
    struct Stalls;

    #[async_trait]
    impl Model for Stalls {
        async fn stream(
            &self,
            _system: &str,
            _messages: &[Message],
            _tools: &[ToolSpec],
            on_text: TextSink<'_>,
        ) -> Result<ModelTurn, TelmoniError> {
            on_text("So far");
            std::future::pending().await
        }
    }

    /// A stalled call ends at the deadline, keeping what the person already
    /// read, rather than running until the console cuts the stream.
    #[tokio::test]
    async fn the_deadline_ends_a_stalled_call_and_keeps_what_was_written() {
        let mut transcript = vec![Message::User("q".into())];
        let outcome = converse(
            &Stalls,
            "sys",
            &mut transcript,
            &[],
            &mut Counting::default(),
            &|_| {},
            Instant::now() + Duration::from_millis(20),
        )
        .await
        .unwrap();
        assert_eq!(outcome.ending, Ending::TimedOut);
        assert_eq!(outcome.text, "So far");
        assert!(outcome.ending.note().is_some());
    }

    #[tokio::test]
    async fn a_reply_cut_at_the_ceiling_says_so_and_a_bare_refusal_fails() {
        let mut transcript = vec![Message::User("q".into())];
        let cut = converse(
            &Stops(StopReason::MaxTokens, "half"),
            "sys",
            &mut transcript,
            &[],
            &mut Counting::default(),
            &|_| {},
            unhurried(),
        )
        .await
        .unwrap();
        assert_eq!(cut.ending, Ending::Truncated);
        assert_eq!(cut.text, "half");
        assert!(cut.ending.note().is_some());

        let refused = converse(
            &Stops(StopReason::Refused, ""),
            "sys",
            &mut transcript,
            &[],
            &mut Counting::default(),
            &|_| {},
            unhurried(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            refused,
            TelmoniError::AgentModelUnavailable { .. }
        ));
    }

    #[test]
    fn the_cap_note_names_the_cap() {
        assert!(CAPPED_NOTE.contains(&format!("{MAX_TOOL_ROUNDS} times")));
    }

    /// The console's own cut-off (`AGENT_STREAM_TIMEOUT_MS`), which the
    /// budget has to leave room under for the save and the `done` event.
    #[test]
    fn the_budget_ends_a_turn_before_the_console_gives_up_on_it() {
        const CONSOLE_STREAM_TIMEOUT: Duration = Duration::from_secs(120);
        assert!(TURN_BUDGET + Duration::from_secs(10) <= CONSOLE_STREAM_TIMEOUT);
    }
}
