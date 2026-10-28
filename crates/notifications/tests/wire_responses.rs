//! Every answer the console reads from notifications, written from its
//! struct to `contract/responses/notifications/` (`make contract`), which the
//! console's schemas parse (`web/lib/server/entities/responses.test.ts`).
#![expect(clippy::expect_used, reason = "test scaffolding")]

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use telmoni_notifications::connector::{ConnectionStatus, Provider};
use telmoni_notifications::db::{
    AttemptLogEntry, AttemptOutcome, AttemptTrigger, ConnectionSummary, DeliveryLogEntry,
    DeliveryStatus, FeedItem,
};
use telmoni_notifications::handler::Feed;
use telmoni_notifications::handler::connectors::{
    Authorize, ConnectionAnswer, ConnectorListing, DeliveryLogPage, SendAnswer, SigningSecret,
};
use telmoni_shared::test_util::pin_contract;
use telmoni_shared::{NotificationKind, Redacted};

const SLACK: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2c01);
const WEBHOOK: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2c02);
const DELIVERY: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2c03);

/// `answer` as `contract/responses/notifications/<name>.json` holds it.
fn pin(name: &str, answer: &impl Serialize) {
    let value = serde_json::to_value(answer).expect("an answer serializes");
    pin_contract(&format!("responses/notifications/{name}.json"), &value);
}

/// A fixed moment, `secs` after the first.
fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000 + secs, 0).expect("a time")
}

fn slack() -> ConnectionSummary {
    ConnectionSummary {
        id: SLACK,
        provider: Provider::Slack,
        external_workspace_id: "T024BE7LD".into(),
        external_workspace_name: Some("Acme".into()),
        channel_id: "C024BE91L".into(),
        channel_name: "#alerts".into(),
        status: ConnectionStatus::Active,
        last_error: None,
        last_delivery_at: Some(at(60)),
        created_at: at(0),
        event_kinds: None,
        previous_secret_expires_at: None,
    }
}

fn webhook() -> ConnectionSummary {
    ConnectionSummary {
        id: WEBHOOK,
        provider: Provider::Webhook,
        external_workspace_id: "hooks.example.com".into(),
        external_workspace_name: None,
        channel_id: String::new(),
        channel_name: "hooks.example.com".into(),
        status: ConnectionStatus::Errored,
        last_error: Some("the endpoint answered 410".into()),
        last_delivery_at: None,
        created_at: at(0),
        event_kinds: Some(vec![
            NotificationKind::IncidentOpened,
            NotificationKind::IncidentResolved,
        ]),
        previous_secret_expires_at: Some(at(86_400)),
    }
}

/// The feed: an incident's notice, unread, and a member's, read.
#[test]
fn the_feed_answers_as_the_console_reads() {
    pin(
        "feed",
        &Feed {
            items: vec![
                FeedItem {
                    id: Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2c04),
                    kind: NotificationKind::IncidentOpened,
                    title: "Slow support runs is alerting".into(),
                    body: "p95 duration 31.3 s, over 30 s".into(),
                    metadata: json!({
                        "incident_id": "0192f3a1-7c2e-7b4d-9a61-3c5e8f0d2b19",
                        "rule_id": "0192f3a1-7c2e-7b4d-9a61-3c5e8f0d2b18",
                    }),
                    read_at: None,
                    created_at: at(60),
                },
                FeedItem {
                    id: Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2c05),
                    kind: NotificationKind::MemberAdded,
                    title: "A member joined".into(),
                    body: "Someone joined the project".into(),
                    metadata: json!({}),
                    read_at: Some(at(30)),
                    created_at: at(0),
                },
            ],
            unread: 1,
        },
    );
}

/// The connectors page: what the deployment can connect, and the project's
/// connections.
#[test]
fn the_connectors_answer_as_the_console_reads() {
    pin(
        "connectors",
        &ConnectorListing {
            enabled: [(Provider::Slack, true), (Provider::Webhook, true)].into(),
            connections: vec![slack(), webhook()],
        },
    );
}

/// A connection as a change to its kinds answers it.
#[test]
fn a_connection_answers_as_the_console_reads() {
    pin(
        "connection",
        &ConnectionAnswer {
            connection: webhook(),
        },
    );
}

/// A webhook's new signing secret, shown once.
#[test]
fn a_signing_secret_answers_as_the_console_reads() {
    pin(
        "signing-secret",
        &SigningSecret {
            connection: webhook(),
            signing_secret: Redacted::from("whsec_not_a_real_secret"),
        },
    );
}

/// A send made now, refused by the far end.
#[test]
fn a_send_answers_as_the_console_reads() {
    pin(
        "sent",
        &SendAnswer {
            delivered: false,
            error: Some("the endpoint answered 410".into()),
        },
    );
}

/// A page of the delivery log, a resend among its attempts.
#[test]
fn the_delivery_log_answers_as_the_console_reads() {
    let attempt = |id, trigger, outcome, status_code, error: Option<&str>| AttemptLogEntry {
        id: Uuid::from_u128(id),
        delivery_id: DELIVERY,
        trigger,
        outcome,
        status_code,
        duration_ms: 182,
        error: error.map(Into::into),
        created_at: at(60),
    };
    pin(
        "deliveries",
        &DeliveryLogPage {
            deliveries: vec![DeliveryLogEntry {
                id: DELIVERY,
                kind: NotificationKind::IncidentOpened,
                subject: "Slow support runs is alerting".into(),
                body: "p95 duration 31.3 s, over 30 s".into(),
                status: DeliveryStatus::Delivered,
                next_attempt_at: at(60),
                last_error: None,
                created_at: at(60),
                updated_at: at(120),
                attempts: vec![
                    attempt(
                        0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2c06,
                        AttemptTrigger::Scheduled,
                        AttemptOutcome::Failed,
                        Some(503),
                        Some("the endpoint answered 503"),
                    ),
                    attempt(
                        0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2c07,
                        AttemptTrigger::Manual,
                        AttemptOutcome::Delivered,
                        Some(200),
                        None,
                    ),
                ],
            }],
            next_before: Some(DELIVERY),
        },
    );
}

/// Where a connect sends the browser.
#[test]
fn a_connect_answers_as_the_console_reads() {
    pin(
        "authorize",
        &Authorize {
            url: "https://slack.com/oauth/v2/authorize?client_id=1&state=s".into(),
            state: "s".into(),
        },
    );
}
