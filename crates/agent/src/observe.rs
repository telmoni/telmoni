//! What a turn reports: each model call and tool call, timed as the loop
//! makes them, and how the question ended — sent, as the turn ends, as a run
//! of the operator's own project with the SDK, the way any agent reports its
//! runs ([`crate::report`]). The model and the tools are wrapped rather than
//! the loop changed, so the loop runs as it did whether anything reports it
//! or not.
//!
//! ⚠ **Ids and counts, never what anyone wrote.** No question, answer,
//! prompt or tool result reaches a record, nor the person who asked. The
//! model quotes members' names and addresses, and an erasure scrubs them by
//! their text from the agent's own tables; it cannot reach a copy kept
//! anywhere else. A question whose answer was withheld for an erasure in its
//! organization is never reported.

use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use telmoni::Telmoni;
use telmoni_shared::acting::Acting;
use telmoni_shared::{OrganizationId, ProjectId, TelmoniError};
use uuid::Uuid;

use crate::config::ModelConfig;
use crate::model::{
    Message, Model, ModelTurn, TextSink, TokenUsage, ToolCall, ToolResult, ToolSpec,
};
use crate::report;
use crate::tools::tool_name;
use crate::turn::{Ending, Runner, StoppedShort};

/// How a console question ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnEnding {
    /// The model answered.
    Answered {
        /// Why the answer ends early when it does, as the person was told in
        /// a note.
        stopped_short: Option<StoppedShort>,
    },
    /// The turn failed.
    Failed {
        /// The problem type, never its detail, which can carry what the
        /// person wrote.
        problem_type: String,
    },
    /// The turn left by a path that named no ending: not an answer, and no
    /// problem the person was shown.
    Unfinished,
    /// The asker left, or is no longer who began it; nothing was saved.
    Cancelled,
}

/// One model call of a console question: what the SDK's record carries of
/// it, and nothing it cannot.
#[derive(Debug, Clone)]
pub(crate) struct Generation {
    /// When it was sent.
    pub(crate) started_at: DateTime<Utc>,
    /// When it ended, or was given up.
    pub(crate) ended_at: DateTime<Utc>,
    /// The provider, as the GenAI conventions name it
    /// ([`ModelConfig::provider_name`]).
    pub(crate) provider: &'static str,
    /// The model that answered as the provider spells it, or the configured
    /// one where the stream named none.
    pub(crate) model: String,
    /// `None` when the provider reported none.
    pub(crate) usage: Option<TokenUsage>,
    /// Why it failed: the kind, never the provider's body, which can echo the
    /// request back.
    pub(crate) error: Option<String>,
}

/// One tool call of a console question.
#[derive(Debug, Clone)]
pub(crate) struct ToolUse {
    /// When it began.
    pub(crate) started_at: DateTime<Utc>,
    /// When it ended, or was given up.
    pub(crate) ended_at: DateTime<Utc>,
    /// The tool, by the agent's name for it, or `unknown` for a name the
    /// model made up, which a passage it read can steer.
    pub(crate) name: &'static str,
    /// Whether it answered the model with an error, or was given up.
    pub(crate) is_error: bool,
}

/// One step of a console question, in the order it happened.
#[derive(Debug, Clone)]
pub(crate) enum Step {
    /// A model call.
    Generation(Generation),
    /// A tool call.
    Tool(ToolUse),
}

/// One console question, once it has ended.
#[derive(Debug, Clone)]
pub(crate) struct TurnRecord {
    /// A fresh id for the question: its run's.
    pub(crate) trace_id: Uuid,
    /// The conversation it was asked in.
    pub(crate) conversation_id: Uuid,
    /// The asker's organization, which the run names as its user.
    pub(crate) organization_id: OrganizationId,
    /// The project it was asked about.
    pub(crate) project_id: Option<ProjectId>,
    /// When the turn began.
    pub(crate) started_at: DateTime<Utc>,
    /// When its outcome was known.
    pub(crate) ended_at: DateTime<Utc>,
    /// How it ended.
    pub(crate) ending: TurnEnding,
    /// Its model and tool calls, in the order they happened.
    pub(crate) steps: Vec<Step>,
}

/// One question as it runs, reported when it drops — on whichever return the
/// turn takes — unless it was withheld.
pub(crate) struct Recorder {
    telmoni: Option<Telmoni>,
    trace: Mutex<Trace>,
}

struct Trace {
    record: TurnRecord,
    ended: bool,
    withheld: bool,
}

impl Recorder {
    /// Begin a question's record; with no SDK, nothing is kept.
    pub(crate) fn start(telmoni: Option<Telmoni>, acting: &Acting, conversation_id: Uuid) -> Self {
        let now = Utc::now();
        Self {
            telmoni,
            trace: Mutex::new(Trace {
                record: TurnRecord {
                    trace_id: Uuid::new_v4(),
                    conversation_id,
                    organization_id: acting.organization_id.clone(),
                    project_id: acting.project.as_ref().map(|p| p.project_id.clone()),
                    started_at: now,
                    ended_at: now,
                    ending: TurnEnding::Cancelled,
                    steps: Vec::new(),
                },
                ended: false,
                withheld: false,
            }),
        }
    }

    /// The model, wrapped so each call is recorded.
    pub(crate) const fn model<'a>(
        &'a self,
        inner: &'a dyn Model,
        config: &'a ModelConfig,
    ) -> RecordingModel<'a> {
        RecordingModel {
            inner,
            recorder: self,
            config,
        }
    }

    /// The tools, wrapped so each call is recorded.
    pub(crate) fn runner<'a>(&'a self, inner: &'a mut dyn Runner) -> RecordingRunner<'a> {
        RecordingRunner {
            inner,
            recorder: self,
        }
    }

    pub(crate) fn answered(&self, ending: Ending) {
        let stopped_short = ending.stopped_short();
        self.end(|record| record.ending = TurnEnding::Answered { stopped_short });
    }

    pub(crate) fn failed(&self, e: &TelmoniError) {
        let problem_type = e.to_problem_details().type_uri;
        self.end(|record| record.ending = TurnEnding::Failed { problem_type });
    }

    pub(crate) fn cancelled(&self) {
        self.end(|record| record.ending = TurnEnding::Cancelled);
    }

    /// The answer is withheld for an erasure in its organization
    /// (`db::erased_during`): nothing of the question goes anywhere, the
    /// report included.
    pub(crate) fn withhold(&self) {
        self.trace().withheld = true;
    }

    /// The question's record as it stands, for the tests to read before the
    /// recorder drops and reports it.
    #[cfg(test)]
    pub(crate) fn record(&self) -> TurnRecord {
        self.trace().record.clone()
    }

    /// The question's end is when its outcome is known, not when the record
    /// is reported, which can come after the exchange is indexed.
    fn end(&self, how: impl FnOnce(&mut TurnRecord)) {
        let mut trace = self.trace();
        how(&mut trace.record);
        trace.record.ended_at = Utc::now();
        trace.ended = true;
    }

    fn trace(&self) -> std::sync::MutexGuard<'_, Trace> {
        self.trace.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, step: Step) {
        if self.telmoni.is_some() {
            self.trace().record.steps.push(step);
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let Some(telmoni) = self.telmoni.take() else {
            return;
        };
        let trace = self.trace.get_mut().unwrap_or_else(PoisonError::into_inner);
        if trace.withheld {
            return;
        }
        let mut record = trace.record.clone();
        if !trace.ended {
            // Every return of the turn names its ending; one that did not
            // left by a path nobody wrote, and is not an answer.
            record.ended_at = Utc::now();
            record.ending = TurnEnding::Unfinished;
        }
        report::send(&telmoni, &record);
    }
}

/// The model, recording each call as it ends — or as the loop gives up on
/// it, at its deadline or when the asker goes.
pub(crate) struct RecordingModel<'a> {
    inner: &'a dyn Model,
    recorder: &'a Recorder,
    /// What `inner` was built from, which names the call's provider and model.
    config: &'a ModelConfig,
}

#[async_trait]
impl Model for RecordingModel<'_> {
    async fn stream(
        &self,
        system: &str,
        transcript: &[Message],
        tools: &[ToolSpec],
        on_text: TextSink<'_>,
    ) -> Result<ModelTurn, TelmoniError> {
        let mut call = PendingGeneration::new(self);
        let answered = self.inner.stream(system, transcript, tools, on_text).await;
        call.finish(&answered);
        answered
    }
}

/// How a model call the loop gave up on — past the turn's deadline, or with
/// the asker gone — is classed: a class, as a failed call's problem type is,
/// never a sentence, since it becomes the span's `error.type`.
const GIVEN_UP: &str = "GivenUp";

/// A model call under way. Recorded when it finishes; one dropped first,
/// which the loop does to a call past its deadline, is recorded as given up.
struct PendingGeneration<'a> {
    recorder: &'a Recorder,
    generation: Option<Generation>,
}

impl<'a> PendingGeneration<'a> {
    fn new(model: &RecordingModel<'a>) -> Self {
        let recorder = model.recorder;
        let config = model.config;
        Self {
            recorder,
            generation: recorder.telmoni.is_some().then(|| Generation {
                started_at: Utc::now(),
                ended_at: Utc::now(),
                provider: config.provider_name(),
                model: config.model.clone(),
                usage: None,
                error: None,
            }),
        }
    }

    fn finish(&mut self, answered: &Result<ModelTurn, TelmoniError>) {
        let Some(mut generation) = self.generation.take() else {
            return;
        };
        generation.ended_at = Utc::now();
        match answered {
            Ok(turn) => {
                if let Some(model) = &turn.model {
                    model.clone_into(&mut generation.model);
                }
                generation.usage = turn.usage;
            }
            // The problem type, as the turn's own ending records one: a
            // kind, never the provider's words, which can echo the request.
            Err(e) => generation.error = Some(e.to_problem_details().type_uri),
        }
        self.recorder.push(Step::Generation(generation));
    }
}

impl Drop for PendingGeneration<'_> {
    fn drop(&mut self) {
        if let Some(mut generation) = self.generation.take() {
            generation.ended_at = Utc::now();
            generation.error = Some(GIVEN_UP.to_owned());
            self.recorder.push(Step::Generation(generation));
        }
    }
}

/// The tools, recording each call as it ends, or as the loop gives up on it.
pub(crate) struct RecordingRunner<'a> {
    inner: &'a mut dyn Runner,
    recorder: &'a Recorder,
}

#[async_trait]
impl Runner for RecordingRunner<'_> {
    async fn run(&mut self, call: &ToolCall) -> ToolResult {
        let mut step = PendingTool::new(self.recorder, call);
        let result = self.inner.run(call).await;
        step.finish(&result);
        result
    }

    async fn recheck(&mut self) {
        self.inner.recheck().await;
    }

    fn cancelled(&self) -> bool {
        self.inner.cancelled()
    }
}

/// A tool call under way, recorded as [`PendingGeneration`] is.
struct PendingTool<'a> {
    recorder: &'a Recorder,
    call: Option<ToolUse>,
}

impl<'a> PendingTool<'a> {
    fn new(recorder: &'a Recorder, call: &ToolCall) -> Self {
        Self {
            recorder,
            call: recorder.telmoni.is_some().then(|| ToolUse {
                started_at: Utc::now(),
                ended_at: Utc::now(),
                name: tool_name(&call.name),
                is_error: false,
            }),
        }
    }

    fn finish(&mut self, result: &ToolResult) {
        if let Some(mut call) = self.call.take() {
            call.ended_at = Utc::now();
            call.is_error = result.is_error;
            self.recorder.push(Step::Tool(call));
        }
    }
}

impl Drop for PendingTool<'_> {
    fn drop(&mut self) {
        if let Some(mut call) = self.call.take() {
            call.ended_at = Utc::now();
            call.is_error = true;
            self.recorder.push(Step::Tool(call));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;
    use telmoni_shared::UserId;
    use tokio::time::{Duration, Instant};

    use super::*;
    use crate::config::Provider;
    use crate::model::StopReason;
    use crate::report::testing::{Capture, attribute, client, spans};
    use crate::turn;

    /// A model that calls the tool it is given once, then answers.
    struct CallsThenAnswers(&'static str, AtomicUsize);

    #[async_trait]
    impl Model for CallsThenAnswers {
        async fn stream(
            &self,
            _: &str,
            _: &[Message],
            _: &[ToolSpec],
            on_text: TextSink<'_>,
        ) -> Result<ModelTurn, TelmoniError> {
            if self.1.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(ModelTurn {
                    tool_calls: vec![ToolCall {
                        id: "c1".into(),
                        name: self.0.into(),
                        input: json!({}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    ..ModelTurn::default()
                });
            }
            on_text("Two members.");
            Ok(ModelTurn {
                text: "Two members.".into(),
                stop_reason: StopReason::Finished,
                usage: Some(TokenUsage {
                    input_tokens: 40,
                    output_tokens: 3,
                    ..Default::default()
                }),
                model: Some("model-x".into()),
                ..ModelTurn::default()
            })
        }
    }

    struct Roster;

    #[async_trait]
    impl Runner for Roster {
        async fn run(&mut self, call: &ToolCall) -> ToolResult {
            ToolResult {
                id: call.id.clone(),
                content: "[1] Members\n<data>\n[{\"name\":\"A\"},{\"name\":\"B\"}]\n</data>".into(),
                is_error: false,
            }
        }
    }

    fn acting() -> Acting {
        Acting {
            user_id: UserId::new(),
            organization_id: OrganizationId::new(),
            organization_role: None,
            project: None,
            session_id: None,
            expires_at: 0,
        }
    }

    /// A model as the tests configure one.
    fn configured(url: &str, model: &str) -> ModelConfig {
        ModelConfig {
            provider: Provider::OpenAi,
            url: url.into(),
            model: model.into(),
            api_key: None,
            max_tokens: 8192,
            timeout_secs: 60,
        }
    }

    /// One question through the recorder on `telmoni`, `then` given the
    /// recorder once it has answered and before it drops and reports.
    async fn ask(telmoni: Telmoni, tool: &'static str, then: impl FnOnce(&Recorder)) {
        let recorder = Recorder::start(Some(telmoni), &acting(), Uuid::new_v4());
        let model = CallsThenAnswers(tool, AtomicUsize::new(0));
        let config = configured("http://model.invalid/v1", "answers");
        let mut runner = Roster;
        let outcome = {
            let watched = recorder.model(&model, &config);
            let mut tools = recorder.runner(&mut runner);
            turn::converse(
                &watched,
                "system",
                &mut vec![Message::User("who is on this project?".into())],
                &[],
                &mut tools,
                &|_| {},
                Instant::now() + Duration::from_secs(5),
            )
            .await
            .unwrap()
        };
        recorder.answered(outcome.ending);
        then(&recorder);
    }

    #[tokio::test]
    async fn each_call_is_a_step_in_order_and_the_run_is_reported_once() {
        let capture = Capture::default();
        ask(client(&capture), "list_members", |recorder| {
            let record = recorder.record();
            assert_eq!(
                record.ending,
                TurnEnding::Answered {
                    stopped_short: None
                }
            );
            let steps: Vec<&str> = record
                .steps
                .iter()
                .map(|step| match step {
                    Step::Generation(_) => "generation",
                    Step::Tool(_) => "tool",
                })
                .collect();
            assert_eq!(
                steps,
                ["generation", "tool", "generation"],
                "the tool between the call that asked for it and the one that answered"
            );
            let Step::Generation(last) = &record.steps[2] else {
                panic!("the last step is the answer");
            };
            assert_eq!(last.model, "model-x");
            assert_eq!(last.usage.map(|u| u.input_tokens), Some(40));
            let Step::Tool(roster) = &record.steps[1] else {
                panic!("the second step is the tool");
            };
            assert_eq!(roster.name, "list_members");
        })
        .await;

        // Reported once the recorder dropped: one export, the run's spans
        // with the tool among them and the run's own last.
        let (_, _, body) = capture.first().await;
        assert_eq!(capture.exports().len(), 1);
        let names: Vec<String> = spans(&body)
            .iter()
            .map(|span| span["name"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert!(
            names.contains(&"execute_tool list_members".to_owned()),
            "{names:?}"
        );
        assert_eq!(
            names.last().map(String::as_str),
            Some("invoke_agent console-agent")
        );
    }

    /// A name the model made up is a name a passage it read can steer: it
    /// is kept as `unknown`, and never reaches the export.
    #[tokio::test]
    async fn a_tool_name_the_model_made_up_is_never_kept() {
        let capture = Capture::default();
        ask(client(&capture), "ada@example.com", |recorder| {
            let record = recorder.record();
            let Step::Tool(ran) = &record.steps[1] else {
                panic!("the second step is the tool");
            };
            assert_eq!(ran.name, "unknown");
        })
        .await;
        let (_, _, body) = capture.first().await;
        assert!(!String::from_utf8_lossy(&body).contains("ada@example.com"));
    }

    /// ⚠ Gemini spoken to over the OpenAI protocol is sent as `gcp.gemini`, as prices name it.
    #[tokio::test]
    async fn a_call_is_sent_under_the_vendor_its_url_is() {
        let capture = Capture::default();
        let recorder = Recorder::start(Some(client(&capture)), &acting(), Uuid::new_v4());
        let config = configured(
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "gemini-3.1-flash-lite",
        );
        let model = CallsThenAnswers("list_members", AtomicUsize::new(0));
        recorder
            .model(&model, &config)
            .stream("system", &[], &[], &|_| {})
            .await
            .unwrap();
        drop(recorder);

        let (_, _, body) = capture.first().await;
        let spans = spans(&body);
        let call = spans.first().expect("the model call's span");
        assert_eq!(
            attribute(call, "gen_ai.provider.name"),
            Some(json!("gcp.gemini"))
        );
        assert_eq!(
            attribute(call, "gen_ai.request.model"),
            Some(json!("gemini-3.1-flash-lite"))
        );
    }

    #[tokio::test]
    async fn a_withheld_answer_is_never_reported() {
        let capture = Capture::default();
        ask(client(&capture), "list_members", Recorder::withhold).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(capture.exports().is_empty());
    }
}
