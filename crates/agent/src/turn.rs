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

/// How often a model call is checked, while it streams, for an asker who
/// has gone.
const RECHECK_EVERY: Duration = if cfg!(test) {
    Duration::from_millis(10)
} else {
    Duration::from_secs(10)
};

/// What the person is told when the turn ran out of time.
const TIMED_OUT_NOTE: &str = "\n\nI ran out of time before finishing. Ask again, or try a \
                              narrower question.";

/// What the person is told when the model broke off mid-answer.
const INTERRUPTED_NOTE: &str = "\n\nThe model stopped answering before it finished. Ask again \
                                for the rest.";

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

    /// Read again whether the asker is still who began the turn, before the
    /// model writes to them again; one found gone shows in
    /// [`Runner::cancelled`].
    async fn recheck(&mut self) {}

    /// Whether the asker has gone — left, or is no longer who began the turn
    /// — so the loop stops spending on them.
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
    /// The model's stream broke off after it had written something.
    Interrupted,
    /// The asker left, or is no longer who began the turn; nothing is saved.
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
            Self::Interrupted => Some(INTERRUPTED_NOTE),
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
        // Before every call after the first: a round that calls no tool
        // would otherwise write to an asker who has gone since the last.
        if round > 0 {
            let Ok(()) = tokio::time::timeout_at(deadline, runner.recheck()).await else {
                return Ok(ended(Ending::TimedOut));
            };
            if runner.cancelled() {
                return Ok(ended(Ending::Cancelled));
            }
        }
        let before = text().len();
        // ⚠ **Watched while it streams.** One call can write for a minute
        // and more; an asker gone meanwhile — signed out everywhere, taken
        // off the project, the panel closed — has it dropped within
        // `RECHECK_EVERY`, not read to its end.
        let streamed =
            tokio::time::timeout_at(deadline, model.stream(system, transcript, tools, &sink));
        let watching = async {
            let mut every = tokio::time::interval(RECHECK_EVERY);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // The first tick is immediate; the asker was read just now.
            every.tick().await;
            loop {
                every.tick().await;
                // Bounded: an auth slow to answer is asked again at the next
                // tick, not waited on, nor asked back to back, and ends nothing.
                let _ = tokio::time::timeout(RECHECK_EVERY, runner.recheck()).await;
                if runner.cancelled() {
                    return;
                }
            }
        };
        let streamed = tokio::select! {
            biased;
            streamed = streamed => streamed,
            () = watching => return Ok(ended(Ending::Cancelled)),
        };
        let Ok(turn) = streamed else {
            return Ok(ended(Ending::TimedOut));
        };
        // A model that breaks off mid-answer leaves words the person has
        // already read; they are kept, with a note, not thrown away with
        // the error. A call that failed before writing anything is an
        // error, even after an earlier round's "let me check": that
        // preamble is no answer to save.
        let turn = match turn {
            Ok(turn) => turn,
            Err(e) if text().len() > before => {
                tracing::warn!(error = %e, "the model broke off mid-answer; keeping what it wrote");
                return Ok(ended(Ending::Interrupted));
            }
            Err(e) => return Err(e),
        };
        if runner.cancelled() {
            return Ok(ended(Ending::Cancelled));
        }
        if turn.stop_reason == StopReason::Refused && turn.text.is_empty() {
            return Err(unavailable("the model declined the request"));
        }
        // A stream that ended without saying it finished: cut off in
        // transit, or not a stream at all (a 200 carrying an error body).
        // Its tool calls may be half-written, so none of them run. As with a
        // broken stream, only words this call wrote make it an answer.
        if turn.stop_reason == StopReason::Unfinished {
            if text().len() > before {
                return Ok(ended(Ending::Interrupted));
            }
            return Err(unavailable("the model's stream ended without an answer"));
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
                StopReason::Unfinished => Ending::Interrupted,
            };
            return Ok(ended(ending));
        }
        if round == MAX_TOOL_ROUNDS {
            return Ok(ended(Ending::Capped));
        }
        let mut results = Vec::with_capacity(turn.tool_calls.len());
        for call in &turn.tool_calls {
            // ⚠ **The lookups are inside the deadline too.** One search can
            // wait minutes on an embeddings endpoint, and a turn past its
            // budget outlives the console's cut-off and the erasure fence's
            // reckoning of how long a turn can still be writing.
            let Ok(result) = tokio::time::timeout_at(deadline, runner.run(call)).await else {
                return Ok(ended(Ending::TimedOut));
            };
            // A lookup that found the asker gone ends the turn here, before
            // the model writes another word to them.
            if runner.cancelled() {
                return Ok(ended(Ending::Cancelled));
            }
            results.push(result);
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
                    stop_reason: StopReason::ToolUse,
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
                stop_reason: StopReason::Finished,
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

    /// A model that writes a few words, then its stream breaks.
    struct Breaks;

    #[async_trait]
    impl Model for Breaks {
        async fn stream(
            &self,
            _system: &str,
            _messages: &[Message],
            _tools: &[ToolSpec],
            on_text: TextSink<'_>,
        ) -> Result<ModelTurn, TelmoniError> {
            on_text("Partly");
            Err(unavailable("stream broke"))
        }
    }

    #[tokio::test]
    async fn a_stream_that_breaks_or_stops_short_keeps_what_was_written() {
        let quiet = |_: &str| {};
        let broke = converse(
            &Breaks,
            "",
            &mut vec![Message::User("q".into())],
            &[],
            &mut Counting(0),
            &quiet,
            unhurried(),
        )
        .await
        .unwrap();
        assert_eq!(broke.ending, Ending::Interrupted);
        assert_eq!(broke.text, "Partly");
        assert!(broke.ending.note().is_some());

        let short = converse(
            &Stops(StopReason::Unfinished, "half an answer"),
            "",
            &mut vec![Message::User("q".into())],
            &[],
            &mut Counting(0),
            &quiet,
            unhurried(),
        )
        .await
        .unwrap();
        assert_eq!(
            short.ending,
            Ending::Interrupted,
            "saved as if it were whole"
        );

        let nothing = converse(
            &Stops(StopReason::Unfinished, ""),
            "",
            &mut vec![Message::User("q".into())],
            &[],
            &mut Counting(0),
            &quiet,
            unhurried(),
        )
        .await;
        assert!(
            nothing.is_err(),
            "a body that was no stream became an empty answer"
        );
    }

    /// A lookup that never answers.
    struct Hangs;

    #[async_trait]
    impl Runner for Hangs {
        async fn run(&mut self, _: &ToolCall) -> ToolResult {
            std::future::pending().await
        }
    }

    /// A tool call that hangs ends the turn at its deadline, keeping what the
    /// model wrote before it, rather than running past the budget.
    #[tokio::test]
    async fn the_deadline_ends_a_hung_lookup_too() {
        let outcome = converse(
            &Looping(AtomicUsize::new(0)),
            "sys",
            &mut vec![Message::User("q".into())],
            &[],
            &mut Hangs,
            &|_| {},
            Instant::now() + Duration::from_millis(20),
        )
        .await
        .unwrap();
        assert_eq!(outcome.ending, Ending::TimedOut);
        assert_eq!(outcome.text, ".");
    }

    /// A lookup that finds the asker no longer who began the turn.
    #[derive(Default)]
    struct FindsThemGone(bool);

    #[async_trait]
    impl Runner for FindsThemGone {
        async fn run(&mut self, call: &ToolCall) -> ToolResult {
            self.0 = true;
            ToolResult {
                id: call.id.clone(),
                content: String::new(),
                is_error: true,
            }
        }

        fn cancelled(&self) -> bool {
            self.0
        }
    }

    /// An asker signed out everywhere, or taken off the project, mid-turn
    /// ends the turn at that lookup: the model is not asked again, so nothing
    /// more reaches the stream.
    #[tokio::test]
    async fn a_lookup_that_finds_the_asker_gone_ends_the_turn() {
        let model = Looping(AtomicUsize::new(0));
        let outcome = converse(
            &model,
            "sys",
            &mut vec![Message::User("q".into())],
            &[],
            &mut FindsThemGone::default(),
            &|_| {},
            unhurried(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.ending, Ending::Cancelled);
        assert_eq!(
            model.0.load(Ordering::SeqCst),
            1,
            "the model was asked again"
        );
    }

    /// Finds the asker gone when it reads them again between rounds.
    #[derive(Default)]
    struct GoneByTheNextRound(bool);

    #[async_trait]
    impl Runner for GoneByTheNextRound {
        async fn run(&mut self, call: &ToolCall) -> ToolResult {
            ToolResult {
                id: call.id.clone(),
                content: "found".into(),
                is_error: false,
            }
        }

        async fn recheck(&mut self) {
            self.0 = true;
        }

        fn cancelled(&self) -> bool {
            self.0
        }
    }

    /// ⚠ An asker gone between rounds hears nothing more: the round that
    /// would have answered without a lookup is never asked for.
    #[tokio::test]
    async fn an_asker_gone_between_rounds_is_not_answered() {
        let model = OnceThenAnswer(AtomicUsize::new(0));
        let outcome = converse(
            &model,
            "sys",
            &mut vec![Message::User("q".into())],
            &[],
            &mut GoneByTheNextRound::default(),
            &|_| {},
            unhurried(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.ending, Ending::Cancelled);
        assert_eq!(
            model.0.load(Ordering::SeqCst),
            1,
            "the answering call was made"
        );
    }

    /// ⚠ An asker gone while the model is still writing has the call dropped
    /// there, not read to its end.
    #[tokio::test]
    async fn an_asker_gone_mid_stream_ends_the_call() {
        let outcome = converse(
            &Stalls,
            "sys",
            &mut vec![Message::User("q".into())],
            &[],
            &mut GoneByTheNextRound::default(),
            &|_| {},
            unhurried(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.ending, Ending::Cancelled);
        assert_eq!(outcome.text, "So far");
    }

    #[test]
    fn the_cap_note_names_the_cap() {
        assert!(CAPPED_NOTE.contains(&format!("{MAX_TOOL_ROUNDS} times")));
    }

    /// The console's own cut-off (`AGENT_STREAM_TIMEOUT_MS`), which the
    /// budget has to leave room under: the last check for an asker gone,
    /// the save's wait on the erasure lock, and the rest of the save and the
    /// `done` event.
    #[test]
    fn the_budget_ends_a_turn_before_the_console_gives_up_on_it() {
        const CONSOLE_STREAM_TIMEOUT: Duration = Duration::from_secs(120);
        const REST_OF_THE_SAVE: Duration = Duration::from_secs(5);
        let after = crate::handler::FINAL_RECHECK + crate::db::SAVE_STATEMENT + REST_OF_THE_SAVE;
        assert!(TURN_BUDGET + after <= CONSOLE_STREAM_TIMEOUT);
    }
}
