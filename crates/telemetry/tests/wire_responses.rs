//! Every answer the console reads from telemetry, written from its struct to
//! `contract/responses/telemetry/` (`make contract`), which the console's
//! schemas parse (`web/lib/server/entities/responses.test.ts`).
#![expect(clippy::expect_used, reason = "test scaffolding")]

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::slug::Slug;
use telmoni_shared::test_util::pin_contract;
use telmoni_shared::{
    ContentMode, IncidentStatus, LayoutDepth, LayoutOpen, MissingData, OrganizationId, ProjectId,
    RuleAggregation, RuleLevel, RuleMeasure, RuleOperator, RuleWindow, SessionSort, Severity,
    SortDirection, SpanKind, SpanStatus, TraceSort, UserSort, ViewKind,
};
use telmoni_telemetry::contract::{InventoryRow, Needs, ReadOn};
use telmoni_telemetry::handler::content_mode::{ContentModeSetting, OFFERED};
use telmoni_telemetry::handler::field_inventory::FieldInventory;
use telmoni_telemetry::handler::health::{Health, HealthIncident, HealthProject};
use telmoni_telemetry::handler::incidents::IncidentList;
use telmoni_telemetry::handler::mutes::MuteList;
use telmoni_telemetry::handler::rules::{Alert, AlertLink, AlertList};
use telmoni_telemetry::handler::session_layout::SessionLayoutSetting;
use telmoni_telemetry::handler::sessions::{SessionDetail, SessionList, SessionTrace};
use telmoni_telemetry::handler::traces::{
    FacetValue, FacetValues, Facets, RunSpend, TraceDetail, TraceHistogram, TraceList, TraceSpan,
    UnpricedModel,
};
use telmoni_telemetry::handler::users::UserList;
use telmoni_telemetry::handler::views::{LayoutFilter, LayoutOptions, View, ViewList, ViewQuery};
use telmoni_telemetry::rules::db::PausedBy;
use telmoni_telemetry::rules::incidents::IncidentRow;
use telmoni_telemetry::rules::mutes::MuteRow;
use telmoni_telemetry::rules::{Definition, Hours, Link, RuleFilters};
use telmoni_telemetry::sessions::SessionRow;
use telmoni_telemetry::traces::{Bar, PinnedRow, TraceRow};
use telmoni_telemetry::users::UserRow;

const RUN: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b17);
const RULE: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b18);
const INCIDENT: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b19);
const CONNECTION: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b1a);
const VIEW: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b1b);
const MEMBER: &str = "user_7Hq2mK9xVb4LpR3s";

/// `answer` as `contract/responses/telemetry/<name>.json` holds it.
fn pin(name: &str, answer: &impl Serialize) {
    let value = serde_json::to_value(answer).expect("an answer serializes");
    pin_contract(&format!("responses/telemetry/{name}.json"), &value);
}

/// A fixed moment, `secs` after the first.
fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000 + secs, 0).expect("a time")
}

fn project() -> ProjectId {
    ProjectId::try_new("project_4kRrX0bQhYd2LmZp").expect("a project id")
}

fn trace() -> TraceRow {
    TraceRow {
        run_id: RUN,
        agent: "support".into(),
        user_id: "u_1f3a".into(),
        session_id: "s_9c2e".into(),
        ended: true,
        status: SpanStatus::Error,
        error_type: "TimeoutError".into(),
        started_at: at(0),
        ended_at: at(4),
        duration_ms: 4_000,
        time_to_first_token_ms: Some(420.0),
        spans: 6,
        steps: 2,
        model_calls: 2,
        tool_calls: 1,
        errors: 1,
        input_tokens: 1_200,
        output_tokens: 340,
        cache_read_input_tokens: 800,
        cache_write_input_tokens: 0,
        models: vec!["gpt-4o-mini".into()],
        tools: vec!["search".into()],
        environments: vec!["production".into()],
        releases: vec!["1.4.2".into()],
    }
}

fn incident() -> IncidentRow {
    IncidentRow {
        id: INCIDENT,
        rule_id: RULE,
        rule_name: "Slow support runs".into(),
        project_id: project(),
        organization_id: OrganizationId::try_new("org_9TzQ1wXe5RkN8bVc").expect("an id"),
        status: IncidentStatus::Acknowledged,
        severity: Severity::Alert,
        value: Some(31_250.0),
        opened_at: at(60),
        acknowledged_at: Some(at(120)),
        acknowledged_by: Some(MEMBER.into()),
        resolved_at: None,
        resolved_by: None,
        reopened: 1,
        told_to: vec![CONNECTION],
        runs: vec![RUN],
    }
}

/// The trace list's page.
#[test]
fn the_trace_list_answers_as_the_console_reads() {
    pin(
        "traces",
        &TraceList {
            traces: vec![trace()],
            days: 7,
            limit: 50,
            page: 1,
            total: 1,
            more: false,
            sort: TraceSort::Started,
            dir: SortDirection::Desc,
        },
    );
}

/// The facets every list's sidebar reads: traces', sessions' and users'.
#[test]
fn the_facets_answer_as_the_console_reads() {
    let values = |value: &str, count| FacetValues {
        values: vec![FacetValue {
            value: value.into(),
            count,
        }],
        more: false,
    };
    pin(
        "facets",
        &Facets {
            days: 7,
            agent: values("support", 12),
            user: values("u_1f3a", 3),
            session: values("s_9c2e", 2),
            status: values("error", 1),
            model: values("gpt-4o-mini", 12),
            tool: values("search", 4),
            environment: values("production", 12),
            release: values("1.4.2", 12),
            tag: values("tier:pro", 5),
            prompt: FacetValues {
                values: Vec::new(),
                more: true,
            },
        },
    );
}

/// The histogram's bars: one runs started in, and one none did.
#[test]
fn the_histogram_answers_as_the_console_reads() {
    pin(
        "histogram",
        &TraceHistogram {
            days: 7,
            bucket_seconds: 3_600,
            utc_offset: -300,
            bars: vec![
                Bar {
                    at: at(0),
                    ok: 10,
                    error: 1,
                    cancelled: 1,
                    tokens: 18_000,
                    duration_p50_ms: Some(1_800.0),
                    duration_p95_ms: Some(4_000.0),
                    time_to_first_token_p50_ms: Some(400.0),
                    time_to_first_token_p95_ms: Some(1_100.0),
                },
                Bar {
                    at: at(3_600),
                    ok: 0,
                    error: 0,
                    cancelled: 0,
                    tokens: 0,
                    duration_p50_ms: None,
                    duration_p95_ms: None,
                    time_to_first_token_p50_ms: None,
                    time_to_first_token_p95_ms: None,
                },
            ],
        },
    );
}

/// One trace's page: a model call that reported its cost, and the run.
#[test]
fn a_trace_answers_as_the_console_reads() {
    let span = |span_id: &str, kind, cost: Option<f64>| TraceSpan {
        span_id: span_id.into(),
        parent_span_id: String::new(),
        trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
        agent: "support".into(),
        kind,
        name: "answer".into(),
        started_at: at(0),
        ended_at: at(4),
        status: SpanStatus::Ok,
        error_type: String::new(),
        model: "gpt-4o-mini".into(),
        provider: "openai".into(),
        input_tokens: 600,
        output_tokens: 170,
        cache_read_input_tokens: 400,
        cache_write_input_tokens: 0,
        cost,
        time_to_first_token: cost.map(|_| 0.42),
        user_id: "u_1f3a".into(),
        session_id: "s_9c2e".into(),
        environment: "production".into(),
        release: "1.4.2".into(),
        tags: [("tier".to_owned(), "pro".to_owned())].into(),
        sample_rate: cost.map(|_| 0.5),
        attributes: [("gen_ai.operation.name".to_owned(), "chat".to_owned())].into(),
        units: 1,
        received_at: at(5),
    };
    pin(
        "trace",
        &TraceDetail {
            run_id: RUN,
            spans: vec![
                span("00f067aa0ba902b7", SpanKind::Run, None),
                span("00f067aa0ba902b8", SpanKind::ModelCall, Some(0.0012)),
            ],
            truncated: false,
            spend: Some(RunSpend {
                spent: 0.0012,
                unpriced: vec![UnpricedModel {
                    provider: "acme".into(),
                    model: "acme-1".into(),
                }],
            }),
        },
    );
}

/// The sessions page's list.
#[test]
fn the_session_list_answers_as_the_console_reads() {
    pin(
        "sessions",
        &SessionList {
            sessions: vec![SessionRow {
                session_id: "s_9c2e".into(),
                user_id: "u_1f3a".into(),
                traces: 2,
                errors: 1,
                steps: 4,
                input_tokens: 2_400,
                output_tokens: 680,
                duration_ms: 4_200,
                first_seen: at(0),
                last_seen: at(90),
            }],
            days: 7,
            limit: 50,
            page: 1,
            total: 1,
            more: false,
            sort: SessionSort::LastSeen,
            dir: SortDirection::Desc,
        },
    );
}

/// One session's page, a span pinned under its trace.
#[test]
fn a_session_answers_as_the_console_reads() {
    pin(
        "session",
        &SessionDetail {
            session_id: "s_9c2e".into(),
            traces: vec![SessionTrace {
                trace: trace(),
                pinned: vec![PinnedRow {
                    run_id: RUN,
                    span_id: "00f067aa0ba902b9".into(),
                    parent_span_id: "00f067aa0ba902b7".into(),
                    kind: SpanKind::Step,
                    name: "plan".into(),
                    started_at: at(1),
                    ended_at: at(2),
                    status: SpanStatus::Ok,
                    error_type: String::new(),
                    model: "gpt-4o-mini".into(),
                    input_tokens: 300,
                    output_tokens: 90,
                }],
            }],
            pinned: vec!["plan".into()],
            days: 7,
            limit: 50,
            more: false,
        },
    );
}

/// The users page's list.
#[test]
fn the_user_list_answers_as_the_console_reads() {
    pin(
        "users",
        &UserList {
            users: vec![UserRow {
                user_id: "u_1f3a".into(),
                traces: 3,
                errors: 1,
                steps: 9,
                input_tokens: 120,
                output_tokens: 40,
                duration_ms: 3_100,
                first_seen: at(0),
                last_seen: at(90),
            }],
            days: 7,
            limit: 50,
            page: 1,
            total: 1,
            more: false,
            sort: UserSort::Tokens,
            dir: SortDirection::Desc,
        },
    );
}

/// Saved views of each kind: the trace list's, the sessions', and a layout.
#[test]
fn saved_views_answer_as_the_console_reads() {
    let view = |kind, name: &str, query, columns: &[&str]| View {
        id: VIEW,
        kind,
        name: name.into(),
        query,
        columns: columns.iter().map(|&column| column.to_owned()).collect(),
        created_at: at(0),
        updated_at: at(60),
    };
    let query = ViewQuery {
        agent: vec!["support".into()],
        status: vec![SpanStatus::Error],
        sort: Some("duration".into()),
        dir: Some(SortDirection::Desc),
        days: Some(7),
        limit: Some(50),
        ..ViewQuery::default()
    };
    let layout = LayoutOptions {
        depth: LayoutDepth::Steps,
        open: LayoutOpen::Failed,
        timeline: true,
        pinned: vec!["plan".into()],
        filter: LayoutFilter {
            kind: vec![SpanKind::ModelCall],
            status: vec![SpanStatus::Error],
            name: vec!["answer".into()],
        },
        messages: true,
        scores: false,
    };
    let query = serde_json::to_value(query).expect("a query serializes");
    let layout = serde_json::to_value(layout).expect("a layout serializes");
    pin(
        "views",
        &ViewList {
            views: vec![view(
                ViewKind::Traces,
                "Failed support runs",
                query,
                &["started", "agent", "tokens"],
            )],
        },
    );
    pin(
        "layouts",
        &ViewList {
            views: vec![view(
                ViewKind::Layout,
                "Steps, failures open",
                layout,
                &["started", "agent", "tokens"],
            )],
        },
    );
    pin(
        "session-views",
        &ViewList {
            views: vec![view(
                ViewKind::Sessions,
                "Slow sessions",
                json!({ "sort": "duration", "dir": "desc" }),
                &["user", "traces", "duration", "last_seen"],
            )],
        },
    );
}

/// The alerts page's alerts: one evaluated and linked within hours, and one
/// its plan paused before a first evaluation.
#[test]
fn alerts_answer_as_the_console_reads() {
    let alert = Alert {
        id: RULE,
        definition: Definition {
            name: "Slow support runs".into(),
            kind: SpanKind::Run,
            measure: RuleMeasure::Duration,
            aggregation: RuleAggregation::P95,
            filters: RuleFilters {
                agent: vec!["support".into()],
                status: vec![SpanStatus::Error],
                ..RuleFilters::default()
            },
            window: RuleWindow::Hour,
            operator: RuleOperator::Gt,
            alert_threshold: 30_000.0,
            warning_threshold: Some(20_000.0),
            missing_data: MissingData::Keep,
            renotify_minutes: Some(60),
        },
        paused: None,
        paused_at: None,
        value: Some(31_250.0),
        evaluated_at: Some(at(60)),
        severity: Severity::Alert,
        severity_since: at(60),
        connectors: vec![AlertLink {
            link: Link {
                connection_id: CONNECTION,
                level: RuleLevel::Warning,
                hours: Some(Hours {
                    days: vec![1, 2, 3, 4, 5],
                    from: "09:00".parse().expect("a time of day"),
                    to: "17:30".parse().expect("a time of day"),
                    utc_offset: -300,
                }),
            },
            muted: false,
        }],
        created_at: at(0),
        updated_at: at(30),
    };
    let paused = Alert {
        id: Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b1e),
        definition: Definition {
            name: "Any failure".into(),
            measure: RuleMeasure::Failures,
            aggregation: RuleAggregation::Count,
            filters: RuleFilters::default(),
            warning_threshold: None,
            renotify_minutes: None,
            ..alert.definition.clone()
        },
        paused: Some(PausedBy::Plan),
        paused_at: Some(at(90)),
        value: None,
        evaluated_at: None,
        severity: Severity::Paused,
        severity_since: at(90),
        connectors: vec![AlertLink {
            link: Link {
                connection_id: CONNECTION,
                level: RuleLevel::Alert,
                hours: None,
            },
            muted: true,
        }],
        created_at: at(0),
        updated_at: at(90),
    };
    pin(
        "alerts",
        &AlertList {
            rules: vec![alert, paused],
        },
    );
}

/// The incidents lane, which a trace's page reads by run too.
#[test]
fn incidents_answer_as_the_console_reads() {
    pin(
        "incidents",
        &IncidentList {
            incidents: vec![incident()],
        },
    );
}

/// Maintenance windows: one alert's, running, and every alert's, ended early.
#[test]
fn mutes_answer_as_the_console_reads() {
    let mute = |id, rule_id, ended_by: Option<&str>| MuteRow {
        id: Uuid::from_u128(id),
        project_id: project(),
        rule_id,
        starts_at: at(0),
        ends_at: at(7_200),
        created_by: MEMBER.into(),
        created_at: at(0),
        ended_by: ended_by.map(Into::into),
    };
    pin(
        "mutes",
        &MuteList {
            mutes: vec![
                mute(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b1c, Some(RULE), None),
                mute(
                    0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b1d,
                    None,
                    Some(MEMBER),
                ),
            ],
        },
    );
}

/// The organization's Overview: its open incidents, each with its project.
#[test]
fn the_organization_health_answers_as_the_console_reads() {
    pin(
        "health",
        &Health {
            incidents: vec![HealthIncident {
                project: HealthProject {
                    id: project(),
                    slug: Slug::from_trusted("support-bot"),
                    name: "Support bot".into(),
                },
                incident: incident(),
            }],
        },
    );
}

/// A project's content mode and the modes its switch offers.
#[test]
fn the_content_mode_answers_as_the_console_reads() {
    pin(
        "content-mode",
        &ContentModeSetting {
            content_mode: ContentMode::Off,
            offered: OFFERED,
        },
    );
}

/// A project's session layout, a saved one named.
#[test]
fn the_session_layout_answers_as_the_console_reads() {
    pin(
        "session-layout",
        &SessionLayoutSetting {
            view_id: Some(VIEW),
        },
    );
}

/// The field inventory, one row of it.
#[test]
fn the_field_inventory_answers_as_the_console_reads() {
    pin(
        "field-inventory",
        &FieldInventory {
            conventions_commit: "a1b2c3d",
            rows: &[InventoryRow {
                attribute: "gen_ai.request.model",
                on: ReadOn::ModelCall,
                kept: "model",
                needs: Needs::AnyMode,
                rule: "at most 256 characters",
            }],
        },
    );
}
