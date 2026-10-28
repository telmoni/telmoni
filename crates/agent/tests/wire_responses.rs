//! Every answer the console reads from the agent, written from its struct, or
//! for a turn's stream from its frames, to `contract/responses/agent/`
//! (`make contract`), which the console parses
//! (`web/lib/server/entities/responses.test.ts`).
#![expect(clippy::expect_used, reason = "test scaffolding")]

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use telmoni_agent::db::{ConversationSummary, MessageRole, StoredMessage};
use telmoni_agent::handler::{Conversation, ConversationList, Status, TurnEvent};
use telmoni_agent::tools::{Citation, Tool};
use telmoni_shared::test_util::pin_contract;
use telmoni_shared::{ProblemDetails, ProjectId};

const CONVERSATION: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2d01);
const ANSWER: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2d03);

/// `answer` as `contract/responses/agent/<name>.json` holds it.
fn pin(name: &str, answer: &impl Serialize) {
    let value = serde_json::to_value(answer).expect("an answer serializes");
    pin_contract(&format!("responses/agent/{name}.json"), &value);
}

/// A fixed moment, `secs` after the first.
fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000 + secs, 0).expect("a time")
}

fn citation() -> Citation {
    Citation {
        index: 1,
        title: "Alerts".into(),
        url: Some("/docs/alerts".into()),
    }
}

fn summary() -> ConversationSummary {
    ConversationSummary {
        id: CONVERSATION,
        title: "Why did the support runs slow down?".into(),
        project_id: ProjectId::try_new("project_4kRrX0bQhYd2LmZp").expect("a project id"),
        created_at: at(0),
        updated_at: at(60),
    }
}

/// Whether the deployment runs the agent.
#[test]
fn the_status_answers_as_the_console_reads() {
    pin("status", &Status { enabled: true });
}

/// The person's conversations.
#[test]
fn the_conversation_list_answers_as_the_console_reads() {
    pin(
        "conversations",
        &ConversationList {
            conversations: vec![summary()],
        },
    );
}

/// One conversation: a question, and an answer citing a page.
#[test]
fn a_conversation_answers_as_the_console_reads() {
    pin(
        "conversation",
        &Conversation {
            conversation: summary(),
            messages: vec![
                StoredMessage {
                    id: Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2d02),
                    role: MessageRole::User,
                    content: "Why did the support runs slow down?".into(),
                    citations: json!([]),
                    created_at: at(0),
                },
                StoredMessage {
                    id: ANSWER,
                    role: MessageRole::Assistant,
                    content: "The p95 rose with the model's latency [1].".into(),
                    citations: json!([citation()]),
                    created_at: at(60),
                },
            ],
        },
    );
}

/// A turn's stream, each event as its frame carries it.
#[test]
fn a_turn_streams_as_the_console_reads() {
    let citation = citation();
    let problem = ProblemDetails::new(
        "/errors/agent/model-unavailable",
        "agent model unavailable",
        502,
    )
    .detail("the model behind the agent did not answer");
    let events = [
        TurnEvent::Conversation {
            conversation_id: CONVERSATION,
        },
        TurnEvent::Tool(Tool::Search.name()),
        TurnEvent::Text("The p95 rose"),
        TurnEvent::Citation(&citation),
        TurnEvent::Done {
            conversation_id: CONVERSATION,
            message_id: ANSWER,
        },
        TurnEvent::Error {
            conversation_id: CONVERSATION,
            problem,
        },
    ];
    let frames: Vec<_> = events
        .iter()
        .map(|event| {
            let (kind, data) = event.frame();
            json!({ "event": kind.as_str(), "data": data })
        })
        .collect();
    pin("turn", &frames);
}
