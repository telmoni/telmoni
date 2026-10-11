//! Dogfood: the console agent's own questions, recorded as runs of the
//! platform's own project ([`telmoni_shared::seam::AgentObserver`]). The
//! agent hands over each question as it ends — ids, counts and timings, never
//! a word anyone wrote — and this module writes it as the rows ingest would:
//! the question a run, each model call and tool call a span under it, the
//! asking organization its user, hashed under the project's own key as any
//! user id is, and the agent's conversation its conversation. In process, through
//! the store, never over a door of ours: a question of ours waits on nothing
//! of ours, and a write that fails is logged by its kind and lost.
//!
//! Nothing is recorded unless `TELEMETRY_DOGFOOD_PROJECT_ID` names the
//! project. The project's content mode is beside the point: the record holds
//! no content to keep or to drop, so what the SDKs call `capture_content` is
//! answered `false` by construction.

use std::sync::Arc;

use chrono::{DateTime, SubsecRound, Utc};
use uuid::Uuid;

use telmoni_shared::seam::{
    AgentGeneration, AgentObserver, AgentStep, AgentToolCall, AgentTurnEnding, AgentTurnRecord,
};
use telmoni_shared::{ProjectId, SpanKind, SpanStatus};

use crate::ingest::span::units_of;
use crate::store::{self, SpanRow};
use crate::{AppState, contract, hashing};

/// The agent every question is a run of: the console agent's own name, which
/// is its monitor's slug once it has one.
pub const AGENT: &str = "console-agent";

/// How a tool call that answered the model with an error, or was given up,
/// is classed: the agent records that it failed and not how.
pub const TOOL_ERROR_TYPE: &str = "ToolError";

/// How a question that left the turn by a path that named no ending is
/// classed.
pub const UNFINISHED_ERROR_TYPE: &str = "Unfinished";

/// The tag that says why an answer ended early, when it did.
pub const STOPPED_SHORT_TAG: &str = "stopped_short";

/// The tag that says what the question was asked about: `project` or
/// `organization`.
pub const ASKED_ABOUT_TAG: &str = "asked_about";

/// The console agent as the platform's own project sees it: the module's
/// state and the project every question is a run of.
pub struct Dogfood {
    state: Arc<AppState>,
    project_id: ProjectId,
}

impl Dogfood {
    /// The observer that records into `project_id`.
    #[must_use]
    pub const fn new(state: Arc<AppState>, project_id: ProjectId) -> Self {
        Self { state, project_id }
    }

    /// Record one question: the project's organization from auth, the
    /// asker's organization hashed under the project's key, and the rows
    /// written. Nothing here reaches the agent: a failure is logged by its
    /// kind and the question is lost to the record, not to the asker.
    pub async fn record(&self, record: AgentTurnRecord) {
        let run_id = record.trace_id;
        let homes = match self
            .state
            .auth
            .project_homes(std::slice::from_ref(&self.project_id))
            .await
        {
            Ok(homes) => homes,
            Err(error) => {
                tracing::warn!(
                    project_id = %self.project_id,
                    error = %error.to_problem_details().type_uri,
                    "dogfood: auth did not answer the project's home; a question was not recorded"
                );
                return;
            }
        };
        let Some(home) = homes.into_iter().next() else {
            tracing::warn!(
                project_id = %self.project_id,
                "dogfood: no project with this id; a question was not recorded"
            );
            return;
        };
        let storage = match hashing::storage_for_project(
            &self.state,
            &self.project_id,
            &home.organization_id,
        )
        .await
        {
            Ok(storage) => storage,
            Err(error) => {
                tracing::warn!(
                    project_id = %self.project_id,
                    error = %error.to_problem_details().type_uri,
                    "dogfood: the project's hashing key was not there; a question was not recorded"
                );
                return;
            }
        };
        let rows = rows(
            &self.project_id,
            &record,
            Utc::now().trunc_subsecs(6),
            &|raw| storage.store(raw),
        );
        if let Err(error) = self.state.store.insert_spans(&rows).await {
            tracing::warn!(
                project_id = %self.project_id,
                kind = store::error_kind(&error),
                "dogfood: ClickHouse did not take a question's spans"
            );
            return;
        }
        tracing::debug!(
            project_id = %self.project_id,
            run_id = %run_id,
            spans = rows.len(),
            "dogfood: a question recorded"
        );
    }
}

impl AgentObserver for Dogfood {
    /// Called on the turn's own task as the question ends: the write goes on
    /// a task of its own, so an answer is never held for a row.
    fn observe(&self, record: AgentTurnRecord) {
        let this = Self {
            state: Arc::clone(&self.state),
            project_id: self.project_id.clone(),
        };
        tokio::spawn(async move { this.record(record).await });
    }
}

/// A question as rows of `spans`: the run first, then each model call and
/// tool call under it, in the order they happened, every `user_id` the
/// asking organization's id as `store_user_id` stores it.
#[must_use]
pub fn rows(
    project_id: &ProjectId,
    record: &AgentTurnRecord,
    received_at: DateTime<Utc>,
    store_user_id: &dyn Fn(&str) -> String,
) -> Vec<SpanRow> {
    let trace_id = record.trace_id.simple().to_string();
    let run_span_id = span_id(record.trace_id);
    let user_id = store_user_id(record.organization_id.as_str());
    let conversation_id = record.conversation_id.to_string();
    // What every span says of where it came from, under the inventory's names.
    let resource = vec![
        ("service.name".to_owned(), "telmoni".to_owned()),
        (
            contract::DISTRO_NAME.to_owned(),
            contract::DISTRO.to_owned(),
        ),
    ];
    let blank =
        |kind: SpanKind, name: String, started_at: DateTime<Utc>, ended_at: DateTime<Utc>| {
            SpanRow {
                project_id: project_id.clone(),
                received_at,
                units: 0,
                run_id: record.trace_id,
                agent: AGENT.to_owned(),
                trace_id: trace_id.clone(),
                span_id: String::new(),
                parent_span_id: run_span_id.clone(),
                kind,
                name,
                started_at,
                ended_at: ended_at.max(started_at),
                status: SpanStatus::Ok,
                error_type: String::new(),
                model: String::new(),
                provider: String::new(),
                input_tokens: 0,
                output_tokens: 0,
                cache_read_input_tokens: 0,
                cache_write_input_tokens: 0,
                user_id: user_id.clone(),
                conversation_id: conversation_id.clone(),
                tags: Vec::new(),
                bound_stall_after_s: None,
                bound_max_steps: None,
                bound_max_cost_usd: None,
                sample_rate: None,
                attributes: resource.clone(),
            }
        };

    let (status, error_type) = match &record.ending {
        AgentTurnEnding::Answered { .. } => (SpanStatus::Ok, String::new()),
        AgentTurnEnding::Failed { problem_type } => (
            SpanStatus::Error,
            capped(problem_type, contract::ERROR_TYPE_MAX),
        ),
        AgentTurnEnding::Unfinished => (SpanStatus::Error, UNFINISHED_ERROR_TYPE.to_owned()),
        AgentTurnEnding::Cancelled => (SpanStatus::Cancelled, String::new()),
    };
    let mut tags = Vec::new();
    if let AgentTurnEnding::Answered {
        stopped_short: Some(why),
    } = &record.ending
    {
        tags.push((STOPPED_SHORT_TAG.to_owned(), why.as_str().to_owned()));
    }
    tags.push((
        ASKED_ABOUT_TAG.to_owned(),
        if record.project_id.is_some() {
            "project".to_owned()
        } else {
            "organization".to_owned()
        },
    ));
    let mut run = SpanRow {
        span_id: run_span_id.clone(),
        parent_span_id: String::new(),
        status,
        error_type,
        tags,
        ..blank(
            SpanKind::Run,
            AGENT.to_owned(),
            record.started_at,
            record.ended_at,
        )
    };
    run.units = units_of(&run);
    let mut out = vec![run];

    for step in &record.steps {
        let mut row = match step {
            AgentStep::Generation(generation) => model_call(generation, &blank),
            AgentStep::Tool(call) => tool_call(call, &blank),
        };
        row.units = units_of(&row);
        out.push(row);
    }
    out
}

/// One model call as a span: its model and provider, its tokens as the
/// conventions count them, how long its first words took, and why it stopped.
fn model_call(
    generation: &AgentGeneration,
    blank: &dyn Fn(SpanKind, String, DateTime<Utc>, DateTime<Utc>) -> SpanRow,
) -> SpanRow {
    let model = capped(&generation.model, contract::MODEL_MAX);
    let name = if model.is_empty() {
        "chat".to_owned()
    } else {
        format!("chat {model}")
    };
    let mut row = blank(
        SpanKind::ModelCall,
        capped(&name, contract::NAME_MAX),
        generation.started_at,
        generation.ended_at,
    );
    row.span_id = span_id(generation.id);
    row.model = model;
    row.provider = capped(generation.provider, contract::PROVIDER_MAX);
    if let Some(usage) = generation.usage {
        row.input_tokens = usage.input_tokens;
        row.output_tokens = usage.output_tokens;
        row.cache_read_input_tokens = usage.cache_read_tokens;
        row.cache_write_input_tokens = usage.cache_write_tokens;
    }
    if let Some(error) = &generation.error {
        row.status = SpanStatus::Error;
        row.error_type = capped(error, contract::ERROR_TYPE_MAX);
    }
    row.attributes.push((
        "gen_ai.request.max_tokens".to_owned(),
        generation.max_tokens.to_string(),
    ));
    if let Some(reason) = generation.stop_reason {
        row.attributes.push((
            "gen_ai.response.finish_reasons".to_owned(),
            serde_json::Value::from(vec![reason]).to_string(),
        ));
    }
    if let Some(first) = generation.first_token_at {
        // Whole milliseconds into a `u32` first: a wait longer than that is
        // no wait a turn survives, and the cast to `f64` is then exact.
        let millis = u32::try_from((first - generation.started_at).num_milliseconds().max(0))
            .unwrap_or(u32::MAX);
        let seconds = f64::from(millis) / 1_000.0;
        row.attributes.push((
            "gen_ai.server.time_to_first_token".to_owned(),
            seconds.to_string(),
        ));
    }
    row
}

/// One tool call as a span: its name, and whether it failed.
fn tool_call(
    call: &AgentToolCall,
    blank: &dyn Fn(SpanKind, String, DateTime<Utc>, DateTime<Utc>) -> SpanRow,
) -> SpanRow {
    let mut row = blank(
        SpanKind::ToolCall,
        capped(call.name, contract::NAME_MAX),
        call.started_at,
        call.ended_at,
    );
    row.span_id = span_id(call.id);
    if call.is_error {
        row.status = SpanStatus::Error;
        row.error_type = TOOL_ERROR_TYPE.to_owned();
    }
    row
}

/// A span's id from the observation's own: the first eight bytes of the
/// UUID, in hex, as the wire spells a span id.
fn span_id(id: Uuid) -> String {
    id.as_bytes()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `text`, cut to `max` characters, as ingest caps the field.
fn capped(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use telmoni_shared::seam::{StoppedShort, TokenUsage};
    use telmoni_shared::{OrganizationId, UserId};

    use super::*;

    fn record(ending: AgentTurnEnding) -> AgentTurnRecord {
        let started = Utc::now().trunc_subsecs(6) - Duration::seconds(10);
        AgentTurnRecord {
            trace_id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            organization_id: OrganizationId::new(),
            project_id: None,
            user_id: UserId::new(),
            started_at: started,
            ended_at: started + Duration::seconds(8),
            ending,
            steps: vec![
                AgentStep::Generation(AgentGeneration {
                    id: Uuid::new_v4(),
                    round: 0,
                    started_at: started,
                    first_token_at: Some(started + Duration::milliseconds(750)),
                    ended_at: started + Duration::seconds(3),
                    provider: "anthropic",
                    model: "claude-sonnet-5-5".into(),
                    max_tokens: 3_000,
                    usage: Some(TokenUsage {
                        input_tokens: 2_000,
                        output_tokens: 120,
                        cache_read_tokens: 1_500,
                        cache_write_tokens: 0,
                        reasoning_tokens: 40,
                    }),
                    stop_reason: Some("tool_use"),
                    error: None,
                    tool_calls: vec!["list_members"],
                }),
                AgentStep::Tool(AgentToolCall {
                    id: Uuid::new_v4(),
                    round: 0,
                    started_at: started + Duration::seconds(3),
                    ended_at: started + Duration::seconds(4),
                    name: "list_members",
                    is_error: true,
                }),
                AgentStep::Generation(AgentGeneration {
                    id: Uuid::new_v4(),
                    round: 1,
                    started_at: started + Duration::seconds(4),
                    first_token_at: None,
                    ended_at: started + Duration::seconds(8),
                    provider: "anthropic",
                    model: "claude-sonnet-5-5".into(),
                    max_tokens: 3_000,
                    usage: None,
                    stop_reason: None,
                    error: Some("/errors/agent/model-unavailable".into()),
                    tool_calls: Vec::new(),
                }),
            ],
        }
    }

    fn hashed(raw: &str) -> String {
        format!("hash({raw})")
    }

    /// The question is the run; each call a span under it, in order; the
    /// asker's organization its user, hashed, and the agent's conversation
    /// its conversation, on every span.
    #[test]
    fn a_question_is_a_run_and_its_calls_are_spans_under_it() {
        let project = ProjectId::new();
        let turn = record(AgentTurnEnding::Answered {
            stopped_short: Some(StoppedShort::Capped),
        });
        let now = Utc::now().trunc_subsecs(6);
        let out = rows(&project, &turn, now, &hashed);
        assert_eq!(out.len(), 4);

        let run = &out[0];
        assert_eq!(run.kind, SpanKind::Run);
        assert_eq!(run.name, AGENT);
        assert_eq!(run.agent, AGENT);
        assert_eq!(run.run_id, turn.trace_id);
        assert_eq!(run.trace_id, turn.trace_id.simple().to_string());
        assert_eq!(run.span_id.len(), 16);
        assert_eq!(run.parent_span_id, "");
        assert_eq!(run.status, SpanStatus::Ok);
        assert_eq!(run.started_at, turn.started_at);
        assert_eq!(run.ended_at, turn.ended_at);
        assert_eq!(run.received_at, now);
        assert_eq!(
            run.user_id,
            format!("hash({})", turn.organization_id),
            "the asking organization, as a user id is stored"
        );
        assert_eq!(run.conversation_id, turn.conversation_id.to_string());
        assert_eq!(
            run.tags,
            [
                (STOPPED_SHORT_TAG.to_owned(), "capped".to_owned()),
                (ASKED_ABOUT_TAG.to_owned(), "organization".to_owned()),
            ]
        );
        assert!(run.units >= 1);

        let first = &out[1];
        assert_eq!(first.kind, SpanKind::ModelCall);
        assert_eq!(first.name, "chat claude-sonnet-5-5");
        assert_eq!(first.model, "claude-sonnet-5-5");
        assert_eq!(first.provider, "anthropic");
        assert_eq!(first.parent_span_id, run.span_id);
        assert_eq!(first.span_id.len(), 16);
        assert_ne!(first.span_id, run.span_id);
        assert_eq!(
            (
                first.input_tokens,
                first.output_tokens,
                first.cache_read_input_tokens,
                first.cache_write_input_tokens
            ),
            (2_000, 120, 1_500, 0)
        );
        assert_eq!(first.status, SpanStatus::Ok);
        assert_eq!(first.user_id, run.user_id);
        assert_eq!(first.conversation_id, run.conversation_id);
        let attribute = |key: &str| {
            first
                .attributes
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(attribute("service.name"), Some("telmoni"));
        assert_eq!(attribute(contract::DISTRO_NAME), Some(contract::DISTRO));
        assert_eq!(attribute("gen_ai.request.max_tokens"), Some("3000"));
        assert_eq!(
            attribute("gen_ai.response.finish_reasons"),
            Some(r#"["tool_use"]"#)
        );
        assert_eq!(attribute("gen_ai.server.time_to_first_token"), Some("0.75"));
        for (key, _) in &first.attributes {
            assert!(
                contract::is_inventory_span(key)
                    || contract::is_inventory_resource(key)
                    || key == "gen_ai.request.max_tokens"
                    || key == "gen_ai.server.time_to_first_token",
                "{key} is an attribute ingest would drop as unlisted"
            );
        }

        let tool = &out[2];
        assert_eq!(tool.kind, SpanKind::ToolCall);
        assert_eq!(tool.name, "list_members");
        assert_eq!(tool.status, SpanStatus::Error);
        assert_eq!(tool.error_type, TOOL_ERROR_TYPE);
        assert_eq!(tool.parent_span_id, run.span_id);

        let failed = &out[3];
        assert_eq!(failed.status, SpanStatus::Error);
        assert_eq!(failed.error_type, "/errors/agent/model-unavailable");
        assert_eq!(failed.input_tokens, 0, "no usage, no tokens");
        assert!(
            !failed
                .attributes
                .iter()
                .any(|(k, _)| k == "gen_ai.response.finish_reasons"),
            "a call that failed stopped for no reason of the model's"
        );
    }

    /// ⚠ Nothing anyone wrote is in a row: no question, answer, prompt or tool
    /// result exists in the record, and what does is ids, names of ours,
    /// counts and types.
    #[test]
    fn a_rows_text_is_names_of_ours_ids_and_types_alone() {
        let project = ProjectId::new();
        let turn = record(AgentTurnEnding::Failed {
            problem_type: "/errors/agent/model-unavailable".into(),
        });
        for row in rows(&project, &turn, Utc::now(), &hashed) {
            for text in [&row.name, &row.error_type, &row.model, &row.provider] {
                assert!(
                    text.len() <= contract::NAME_MAX,
                    "{text} is longer than a field"
                );
                assert!(!text.contains(' ') || text.starts_with("chat "), "{text}");
            }
            assert!(
                row.tags
                    .iter()
                    .all(|(k, v)| k.len() <= 64 && v.len() <= 256)
            );
        }
    }

    #[test]
    fn each_ending_is_a_status_and_a_type() {
        let project = ProjectId::new();
        let now = Utc::now();
        let status = |ending| {
            let run = rows(&project, &record(ending), now, &hashed).swap_remove(0);
            (run.status, run.error_type, run.tags)
        };
        let (ok, none, tags) = status(AgentTurnEnding::Answered {
            stopped_short: None,
        });
        assert_eq!((ok, none.as_str()), (SpanStatus::Ok, ""));
        assert_eq!(
            tags,
            [(ASKED_ABOUT_TAG.to_owned(), "organization".to_owned())]
        );
        let (error, problem, _) = status(AgentTurnEnding::Failed {
            problem_type: "/errors/agent/budget".into(),
        });
        assert_eq!(
            (error, problem.as_str()),
            (SpanStatus::Error, "/errors/agent/budget")
        );
        let (error, unfinished, _) = status(AgentTurnEnding::Unfinished);
        assert_eq!(
            (error, unfinished.as_str()),
            (SpanStatus::Error, UNFINISHED_ERROR_TYPE)
        );
        let (cancelled, none, _) = status(AgentTurnEnding::Cancelled);
        assert_eq!((cancelled, none.as_str()), (SpanStatus::Cancelled, ""));

        let mut about_project = record(AgentTurnEnding::Cancelled);
        about_project.project_id = Some(ProjectId::new());
        let run = rows(&project, &about_project, now, &hashed).swap_remove(0);
        assert_eq!(
            run.tags,
            [(ASKED_ABOUT_TAG.to_owned(), "project".to_owned())]
        );
    }

    #[test]
    fn a_field_past_its_cap_is_cut_not_refused() {
        let project = ProjectId::new();
        let mut turn = record(AgentTurnEnding::Answered {
            stopped_short: None,
        });
        if let AgentStep::Generation(g) = &mut turn.steps[0] {
            g.model = "m".repeat(contract::MODEL_MAX + 20);
        }
        let call = &rows(&project, &turn, Utc::now(), &hashed)[1];
        assert_eq!(call.model.chars().count(), contract::MODEL_MAX);
        assert_eq!(call.name.chars().count(), contract::NAME_MAX);
    }
}
