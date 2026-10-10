//! What a turn hands whatever records the agent's questions
//! ([`AgentObserver`]): each model call and tool call, timed as the loop
//! makes them, and how the question ended. The model and the tools are
//! wrapped rather than the loop changed, so the loop runs as it did whether
//! anything records it or not.

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use telmoni_shared::TelmoniError;
use telmoni_shared::acting::Acting;
use telmoni_shared::seam::{
    AgentGeneration, AgentObserver, AgentStep, AgentToolCall, AgentTurnEnding, AgentTurnRecord,
};
use uuid::Uuid;

use crate::config::ModelConfig;
use crate::model::{Message, Model, ModelTurn, TextSink, ToolCall, ToolResult, ToolSpec};
use crate::tools::tool_name;
use crate::turn::{Ending, Runner};

/// One question as it runs, handed to the observer when it drops — on
/// whichever return the turn takes — unless it was withheld.
pub(crate) struct Recorder {
    observer: Option<Arc<dyn AgentObserver>>,
    trace: Mutex<Trace>,
}

struct Trace {
    record: AgentTurnRecord,
    generations: usize,
    ended: bool,
    withheld: bool,
}

impl Recorder {
    /// Begin a question's record; with no observer, nothing is kept.
    pub(crate) fn start(
        observer: Option<Arc<dyn AgentObserver>>,
        acting: &Acting,
        conversation_id: Uuid,
    ) -> Self {
        let now = Utc::now();
        Self {
            observer,
            trace: Mutex::new(Trace {
                record: AgentTurnRecord {
                    trace_id: Uuid::new_v4(),
                    conversation_id,
                    organization_id: acting.organization_id.clone(),
                    project_id: acting.project.as_ref().map(|p| p.project_id.clone()),
                    user_id: acting.user_id.clone(),
                    started_at: now,
                    ended_at: now,
                    ending: AgentTurnEnding::Cancelled,
                    steps: Vec::new(),
                },
                generations: 0,
                ended: false,
                withheld: false,
            }),
        }
    }

    /// The model, wrapped so each call is recorded.
    pub(crate) const fn model<'a>(
        &'a self,
        inner: &'a dyn Model,
        config: Option<&'a ModelConfig>,
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
        self.end(|record| record.ending = AgentTurnEnding::Answered { stopped_short });
    }

    pub(crate) fn failed(&self, e: &TelmoniError) {
        let problem_type = e.to_problem_details().type_uri;
        self.end(|record| record.ending = AgentTurnEnding::Failed { problem_type });
    }

    pub(crate) fn cancelled(&self) {
        self.end(|record| record.ending = AgentTurnEnding::Cancelled);
    }

    /// The answer is withheld for an erasure in its organization
    /// (`db::erased_during`): nothing of the question goes anywhere, the
    /// observer included.
    pub(crate) fn withhold(&self) {
        self.trace().withheld = true;
    }

    /// The question's end is when its outcome is known, not when the record
    /// is handed over, which can come after the exchange is indexed.
    fn end(&self, how: impl FnOnce(&mut AgentTurnRecord)) {
        let mut trace = self.trace();
        how(&mut trace.record);
        trace.record.ended_at = Utc::now();
        trace.ended = true;
    }

    fn trace(&self) -> std::sync::MutexGuard<'_, Trace> {
        self.trace.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The round of the model call being made now, from 0.
    fn next_round(&self) -> usize {
        let mut trace = self.trace();
        let round = trace.generations;
        trace.generations += 1;
        round
    }

    /// The round whose model call asked for the tools running now.
    fn current_round(&self) -> usize {
        self.trace().generations.saturating_sub(1)
    }

    fn push(&self, step: AgentStep) {
        if self.observer.is_some() {
            self.trace().record.steps.push(step);
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let Some(observer) = self.observer.take() else {
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
            record.ending = AgentTurnEnding::Unfinished;
        }
        observer.observe(record);
    }
}

/// The model, recording each call as it ends — or as the loop gives up on
/// it, at its deadline or when the asker goes.
pub(crate) struct RecordingModel<'a> {
    inner: &'a dyn Model,
    recorder: &'a Recorder,
    config: Option<&'a ModelConfig>,
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
        let first = Mutex::new(None::<DateTime<Utc>>);
        let sink = |delta: &str| {
            first
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_or_insert_with(Utc::now);
            on_text(delta);
        };
        let answered = self.inner.stream(system, transcript, tools, &sink).await;
        call.first_token_at = *first.lock().unwrap_or_else(PoisonError::into_inner);
        call.finish(&answered);
        answered
    }
}

/// A model call under way. Recorded when it finishes; one dropped first,
/// which the loop does to a call past its deadline, is recorded as given up.
struct PendingGeneration<'a> {
    recorder: &'a Recorder,
    generation: Option<AgentGeneration>,
    first_token_at: Option<DateTime<Utc>>,
}

impl<'a> PendingGeneration<'a> {
    fn new(model: &RecordingModel<'a>) -> Self {
        let recorder = model.recorder;
        let config = model.config;
        Self {
            recorder,
            generation: recorder.observer.is_some().then(|| AgentGeneration {
                id: Uuid::new_v4(),
                round: recorder.next_round(),
                started_at: Utc::now(),
                first_token_at: None,
                ended_at: Utc::now(),
                provider: config.map_or("unknown", |c| c.provider.as_str()),
                model: config.map(|c| c.model.clone()).unwrap_or_default(),
                max_tokens: config.map_or(0, |c| c.max_tokens),
                usage: None,
                stop_reason: None,
                error: None,
                tool_calls: Vec::new(),
            }),
            first_token_at: None,
        }
    }

    fn finish(&mut self, answered: &Result<ModelTurn, TelmoniError>) {
        let Some(mut generation) = self.generation.take() else {
            return;
        };
        generation.ended_at = Utc::now();
        generation.first_token_at = self.first_token_at;
        match answered {
            Ok(turn) => {
                if let Some(model) = &turn.model {
                    model.clone_into(&mut generation.model);
                }
                generation.usage = turn.usage;
                generation.stop_reason = Some(turn.stop_reason.as_str());
                generation.tool_calls =
                    turn.tool_calls.iter().map(|c| tool_name(&c.name)).collect();
            }
            Err(e) => generation.error = Some(e.to_string()),
        }
        self.recorder.push(AgentStep::Generation(generation));
    }
}

impl Drop for PendingGeneration<'_> {
    fn drop(&mut self) {
        if let Some(mut generation) = self.generation.take() {
            generation.ended_at = Utc::now();
            generation.first_token_at = self.first_token_at;
            generation.error = Some("given up: the turn's deadline, or the asker gone".to_owned());
            self.recorder.push(AgentStep::Generation(generation));
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
    call: Option<AgentToolCall>,
}

impl<'a> PendingTool<'a> {
    fn new(recorder: &'a Recorder, call: &ToolCall) -> Self {
        Self {
            recorder,
            call: recorder.observer.is_some().then(|| AgentToolCall {
                id: Uuid::new_v4(),
                round: recorder.current_round(),
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
            self.recorder.push(AgentStep::Tool(call));
        }
    }
}

impl Drop for PendingTool<'_> {
    fn drop(&mut self) {
        if let Some(mut call) = self.call.take() {
            call.ended_at = Utc::now();
            call.is_error = true;
            self.recorder.push(AgentStep::Tool(call));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;
    use telmoni_shared::{OrganizationId, UserId};
    use tokio::time::{Duration, Instant};

    use super::*;
    use crate::model::StopReason;
    use crate::turn;

    /// An observer that keeps what it is handed.
    #[derive(Default)]
    struct Kept {
        records: Mutex<Vec<AgentTurnRecord>>,
    }

    impl AgentObserver for Kept {
        fn observe(&self, record: AgentTurnRecord) {
            self.records.lock().unwrap().push(record);
        }
    }

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
                usage: Some(telmoni_shared::seam::TokenUsage {
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

    async fn ask(observer: Arc<Kept>, tool: &'static str, then: impl FnOnce(&Recorder)) {
        let recorder = Recorder::start(
            Some(observer as Arc<dyn AgentObserver>),
            &acting(),
            Uuid::new_v4(),
        );
        let model = CallsThenAnswers(tool, AtomicUsize::new(0));
        let mut runner = Roster;
        let outcome = {
            let watched = recorder.model(&model, None);
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
    async fn each_call_is_a_step_in_order_and_the_record_is_handed_over_once() {
        let observer = Arc::new(Kept::default());
        ask(observer.clone(), "list_members", |_| {}).await;
        let records = observer.records.lock().unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(
            record.ending,
            AgentTurnEnding::Answered {
                stopped_short: None
            }
        );
        let steps: Vec<(&str, usize)> = record
            .steps
            .iter()
            .map(|step| match step {
                AgentStep::Generation(g) => ("generation", g.round),
                AgentStep::Tool(t) => ("tool", t.round),
            })
            .collect();
        assert_eq!(
            steps,
            [("generation", 0), ("tool", 0), ("generation", 1)],
            "a tool belongs to the round that asked for it"
        );
        let AgentStep::Generation(last) = &record.steps[2] else {
            panic!("the last step is the answer");
        };
        assert_eq!(last.model, "model-x");
        assert_eq!(last.usage.map(|u| u.input_tokens), Some(40));
        assert!(last.first_token_at.is_some());
        let AgentStep::Tool(roster) = &record.steps[1] else {
            panic!("the second step is the tool");
        };
        assert_eq!(roster.name, "list_members");
    }

    #[tokio::test]
    async fn a_tool_name_the_model_made_up_is_never_kept() {
        let observer = Arc::new(Kept::default());
        ask(observer.clone(), "ada@example.com", |_| {}).await;
        let records = observer.records.lock().unwrap();
        let AgentStep::Generation(asked) = &records[0].steps[0] else {
            panic!("the first step is a model call");
        };
        let AgentStep::Tool(ran) = &records[0].steps[1] else {
            panic!("the second step is the tool");
        };
        assert_eq!(asked.tool_calls, ["unknown"]);
        assert_eq!(ran.name, "unknown");
    }

    #[tokio::test]
    async fn a_withheld_answer_is_never_handed_over() {
        let observer = Arc::new(Kept::default());
        ask(observer.clone(), "list_members", Recorder::withhold).await;
        assert!(observer.records.lock().unwrap().is_empty());
    }
}
