//! Every answer the console reads from telemetry, written from its struct to
//! `contract/responses/telemetry/` (`make contract`), which the console's
//! schemas parse (`web/lib/server/entities/responses.test.ts`).
#![expect(clippy::expect_used, reason = "test scaffolding")]

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use uuid::Uuid;

use telmoni_shared::test_util::pin_contract;
use telmoni_shared::{
    ChangeKind, ChartMeasure, ContentMode, IncidentStatus, MetricsGroup, MissingData, MonitorCause,
    OrganizationId, ProjectId, RuleAggregation, RuleLevel, RuleMeasure, RuleOperator, RuleWindow,
    Severity, SortDirection, SpanKind, SpanStatus, TraceSort,
};
use telmoni_telemetry::changes::Change;
use telmoni_telemetry::contract::{InventoryRow, Needs, ReadOn};
use telmoni_telemetry::handler::agents::{AGENTS_DAYS, AgentRow, Agents};
use telmoni_telemetry::handler::changes::ChangeList;
use telmoni_telemetry::handler::content_mode::{ContentModeSetting, OFFERED};
use telmoni_telemetry::handler::field_inventory::{FieldInventory, Row};
use telmoni_telemetry::handler::incidents::{IncidentAnswer, IncidentList};
use telmoni_telemetry::handler::mutes::MuteList;
use telmoni_telemetry::handler::privacy::{DoorScrub, Posture, Retention, Sent};
use telmoni_telemetry::handler::rules::{Alert, AlertLink, AlertList};
use telmoni_telemetry::handler::traces::{
    FacetValue, FacetValues, Facets, SpanFields, TraceDetail, TraceHistogram, TraceList, TraceSpan,
    TraceSpend, UnpricedModel,
};
use telmoni_telemetry::handler::versions::Versions;
use telmoni_telemetry::handler::views::{DashboardChart, DashboardOptions, View, ViewList};
use telmoni_telemetry::rules::db::PausedBy;
use telmoni_telemetry::rules::incidents::IncidentRow;
use telmoni_telemetry::rules::mutes::MuteRow;
use telmoni_telemetry::rules::{Definition, Hours, Link, RuleFilters};
use telmoni_telemetry::traces::{Bar, ListSort, TraceRow};
use telmoni_telemetry::versions::{Choice, ScoreMean, Version};

/// A trace's agent span's turn, as content keeps it: the person's message and
/// the agent's answer, in the conventions' parts.
fn turn() -> Vec<SpanFields> {
    vec![SpanFields {
        trace_id: TRACE.into(),
        span_id: "00f067aa0ba902b7".into(),
        fields: [
            (
                "gen_ai.input.messages".to_owned(),
                r#"[{"role":"user","parts":[{"type":"text","content":"Summarize the report"}]}]"#
                    .to_owned(),
            ),
            (
                "gen_ai.output.messages".to_owned(),
                r#"[{"role":"assistant","parts":[{"type":"text","content":"**Revenue** rose 4%."}],"finish_reason":"stop"}]"#
                    .to_owned(),
            ),
        ]
        .into(),
    }]
}

const TRACE: &str = "0192f3a17c2e7b4d9a613c5e8f0d2b17";
const RULE: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b18);
const INCIDENT: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b19);
const MONITOR: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b29);
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
        trace_id: TRACE.into(),
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
        retrievals: 1,
        errors: 1,
        input_tokens: 1_200,
        output_tokens: 340,
        cache_read_input_tokens: 800,
        cache_write_input_tokens: 0,
        reasoning_output_tokens: 0,
        spend: 0.0042,
        models: vec!["gpt-4o-mini".into()],
        tools: vec!["search".into()],
        environments: vec!["production".into()],
        releases: vec!["1.4.2".into()],
        tags: vec!["repo:telmoni".into()],
    }
}

fn incident() -> IncidentRow {
    IncidentRow {
        id: INCIDENT,
        rule_id: Some(RULE),
        rule_name: Some("Slow support traces".into()),
        monitor_id: None,
        agent: None,
        cause: None,
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
        traces: vec![TRACE.into()],
    }
}

/// The trace list's page.
#[test]
fn the_trace_list_answers_as_the_console_reads() {
    pin(
        "traces",
        &telmoni_telemetry::handler::traces::ScoredTraceList {
            list: TraceList {
                traces: vec![trace()],
                days: 7,
                limit: 50,
                page: 1,
                total: 1,
                more: false,
                sort: ListSort::Column(TraceSort::Started),
                dir: SortDirection::Desc,
            },
            scores: [(
                TRACE.to_owned(),
                vec![telmoni_telemetry::scores::Summary {
                    name: "helpful".into(),
                    data_type: telmoni_shared::ScoreType::Number,
                    mean: Some(0.8),
                    count: 1,
                    categories: Vec::new(),
                }],
            )]
            .into(),
        },
    );
}

/// Each kind, including a capped model and a tool's first call.
fn changes() -> Vec<Change> {
    let change = |kind, prompt: Option<&str>, value: &str, secs, capped| Change {
        agent: "support".into(),
        environment: "production".into(),
        kind,
        prompt: prompt.map(Into::into),
        value: value.into(),
        changed_at: at(secs),
        trace_id: TRACE.into(),
        capped,
    };
    vec![
        change(ChangeKind::Tool, None, "delete_user", 180, false),
        change(ChangeKind::Prompt, Some("support-reply"), "3", 120, false),
        change(ChangeKind::Model, None, "gpt-4o-mini-2024-07-18", 60, true),
        change(ChangeKind::Release, None, "1.4.2", 0, false),
    ]
}

/// The changes the trace list's charts mark, the newest first.
#[test]
fn the_changes_answer_as_the_console_reads() {
    pin(
        "changes",
        &ChangeList {
            days: 7,
            more: false,
            changes: changes(),
        },
    );
}

/// The Agents page's rows: one agent whose last trace ended and was priced,
/// with its newest change of each kind, and one still on its first trace,
/// with no usual cost yet.
#[test]
fn the_agents_answer_as_the_console_reads() {
    pin(
        "agents",
        &Agents {
            days: AGENTS_DAYS,
            agents: vec![
                AgentRow {
                    agent: "support".into(),
                    traces: 12,
                    failed_traces: 1,
                    last_trace_id: TRACE.into(),
                    last_started_at: at(0),
                    last_ended: true,
                    last_status: SpanStatus::Ok,
                    last_spend: 0.042,
                    usual_spend: Some(0.031),
                },
                AgentRow {
                    agent: "nightly-report".into(),
                    traces: 1,
                    failed_traces: 0,
                    last_trace_id: "0192f3a17c2e7b4d9a613c5e8f0d2b2c".into(),
                    last_started_at: at(60),
                    last_ended: false,
                    last_status: SpanStatus::Ok,
                    last_spend: 0.0,
                    usual_spend: None,
                },
            ],
            changes: changes(),
        },
    );
}

/// An agent's releases side by side: the newer failing more and scoring
/// lower, one whose only trace has not ended, and the traces that named none.
#[test]
fn the_versions_answer_as_the_console_reads() {
    let choice = |value: &str, traces| Choice {
        value: value.into(),
        traces,
    };
    let score = |mean, count| {
        vec![ScoreMean {
            name: "helpful".into(),
            mean,
            count,
        }]
    };
    pin(
        "agent-versions",
        &Versions {
            days: AGENTS_DAYS,
            by: ChangeKind::Release,
            environment: "production".into(),
            environments: vec![choice("production", 23), choice("", 2)],
            prompt: None,
            prompts: Vec::new(),
            versions: vec![
                Version {
                    value: "1.4.3".into(),
                    first_at: at(7_200),
                    traces: 1,
                    failed_traces: 0,
                    p50_ms: None,
                    p95_ms: None,
                    median_spend: None,
                    scores: Vec::new(),
                },
                Version {
                    value: "1.4.2".into(),
                    first_at: at(3_600),
                    traces: 10,
                    failed_traces: 4,
                    p50_ms: Some(2_400),
                    p95_ms: Some(9_100),
                    median_spend: Some(0.042),
                    scores: score(0.6, 5),
                },
                Version {
                    value: "1.4.1".into(),
                    first_at: at(0),
                    traces: 10,
                    failed_traces: 2,
                    p50_ms: Some(2_100),
                    p95_ms: Some(6_000),
                    median_spend: Some(0.031),
                    scores: score(0.8, 5),
                },
                Version {
                    value: String::new(),
                    first_at: at(60),
                    traces: 2,
                    failed_traces: 0,
                    p50_ms: Some(1_900),
                    p95_ms: Some(1_900),
                    median_spend: Some(0.0),
                    scores: Vec::new(),
                },
            ],
            scores: vec!["helpful".into()],
            more: false,
        },
    );
}

/// The facets the trace list's sidebar reads.
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
            fault: values("rate_limited", 1),
            days: 7,
            agent: values("support", 12),
            user: values("u_1f3a", 3),
            session: values("s_9c2e", 2),
            status: values("error", 1),
            model: values("gpt-4o-mini", 12),
            tool: values("search", 4),
            environment: values("production", 12),
            release: values("1.4.2", 12),
            tag: values("team:search", 5),
            prompt: FacetValues {
                values: Vec::new(),
                more: true,
            },
            prompt_version: values("support-reply:3", 4),
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
            score: Some("relevance".into()),
            bars: vec![
                Bar {
                    at: at(0),
                    ok: 10,
                    error: 1,
                    cancelled: 1,
                    faults: [("rate_limited".into(), 1)].into(),
                    tokens: 18_000,
                    tokens_by_model: [(
                        "gpt-4o-mini".into(),
                        telmoni_telemetry::traces::TokenCounts {
                            input_tokens: 12_000,
                            output_tokens: 6_000,
                            cache_read_input_tokens: 3_000,
                            cache_write_input_tokens: 1_000,
                            reasoning_output_tokens: 2_000,
                        },
                    )]
                    .into(),
                    spend: 0.063,
                    output_speed_by_model: [
                        (
                            "gpt-4o-mini".into(),
                            telmoni_telemetry::traces::OutputSpeed {
                                p50: Some(50.0),
                                p05: Some(5.0),
                            },
                        ),
                        (
                            "untimed".into(),
                            telmoni_telemetry::traces::OutputSpeed::default(),
                        ),
                    ]
                    .into(),
                    duration_p50_ms: Some(1_800.0),
                    duration_p95_ms: Some(4_000.0),
                    time_to_first_token_p50_ms: Some(400.0),
                    time_to_first_token_p95_ms: Some(1_100.0),
                    score_mean: Some(0.82),
                },
                Bar {
                    at: at(3_600),
                    ok: 0,
                    error: 0,
                    cancelled: 0,
                    faults: Default::default(),
                    tokens: 0,
                    tokens_by_model: Default::default(),
                    output_speed_by_model: Default::default(),
                    spend: 0.0,
                    duration_p50_ms: None,
                    duration_p95_ms: None,
                    time_to_first_token_p50_ms: None,
                    time_to_first_token_p95_ms: None,
                    score_mean: None,
                },
            ],
        },
    );
}

/// One trace's page: a model call that reported its cost, a retrieval, and the agent span.
#[test]
fn a_trace_answers_as_the_console_reads() {
    let span = |span_id: &str, kind, cost: Option<f64>| TraceSpan {
        fault: None,
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
        reasoning_output_tokens: 20,
        cost,
        time_to_first_token: cost.map(|_| 0.42),
        output_speed: cost.map(|_| 169.0 / (4.0 - 0.42)),
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
    let found = TraceSpan {
        name: "docs-index".into(),
        model: String::new(),
        provider: String::new(),
        input_tokens: 0,
        output_tokens: 0,
        cache_read_input_tokens: 0,
        attributes: [
            ("gen_ai.data_source.id", "docs-index"),
            ("gen_ai.retrieval.top_k", "5"),
            ("telmoni.retrieval.returned", "3"),
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .into(),
        ..span("00f067aa0ba902b9", SpanKind::Retrieval, None)
    };
    pin(
        "trace",
        &TraceDetail {
            trace_id: TRACE.into(),
            spans: vec![
                span("00f067aa0ba902b7", SpanKind::Agent, None),
                found,
                span("00f067aa0ba902b8", SpanKind::ModelCall, Some(0.0012)),
            ],
            truncated: false,
            spend: Some(TraceSpend {
                spent: 0.0012,
                unpriced: vec![UnpricedModel {
                    provider: "acme".into(),
                    model: "acme-1".into(),
                }],
            }),
            content: Some(turn()),
        },
    );
}

/// The project's saved dashboards.
#[test]
fn saved_dashboards_answer_as_the_console_reads() {
    let charts = DashboardOptions {
        charts: vec![
            DashboardChart {
                measure: ChartMeasure::Spend,
                group: Some(MetricsGroup::Tag),
                tag: Some("team".into()),
            },
            DashboardChart {
                measure: ChartMeasure::Latency,
                group: None,
                tag: None,
            },
        ],
    };
    pin(
        "dashboards",
        &ViewList {
            views: vec![View {
                id: VIEW,
                name: "Spend by team".into(),
                query: serde_json::to_value(charts).expect("a dashboard serializes"),
                created_at: at(0),
                updated_at: at(60),
            }],
        },
    );
}

/// The alerts page's alerts: one evaluated and linked within hours, one its
/// plan paused before a first evaluation, a starter beside one the plan had
/// no room for, and one on a retrieval's source.
#[test]
fn alerts_answer_as_the_console_reads() {
    let alert = Alert {
        id: RULE,
        definition: Definition {
            name: "Slow support traces".into(),
            kind: SpanKind::Agent,
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
            score_name: None,
            baseline: telmoni_shared::RuleBaseline::None,
            objective: None,
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
        starter: false,
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
            score_name: None,
            baseline: telmoni_shared::RuleBaseline::None,
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
        starter: false,
        created_at: at(0),
        updated_at: at(90),
    };
    let starter = Alert {
        id: Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b1f),
        definition: Definition {
            name: "Failing traces".into(),
            measure: RuleMeasure::Failures,
            aggregation: RuleAggregation::Avg,
            filters: RuleFilters::default(),
            alert_threshold: 0.1,
            warning_threshold: Some(0.05),
            missing_data: MissingData::Zero,
            renotify_minutes: None,
            ..alert.definition.clone()
        },
        paused: None,
        paused_at: None,
        value: Some(0.0),
        evaluated_at: Some(at(300)),
        severity: Severity::Ok,
        severity_since: at(0),
        connectors: Vec::new(),
        starter: true,
        created_at: at(0),
        updated_at: at(0),
    };
    let retrieval = Alert {
        id: Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b20),
        definition: Definition {
            name: "Failing docs searches".into(),
            kind: SpanKind::Retrieval,
            measure: RuleMeasure::Failures,
            aggregation: RuleAggregation::Count,
            filters: RuleFilters {
                source: vec!["docs-index".into()],
                ..RuleFilters::default()
            },
            alert_threshold: 5.0,
            warning_threshold: None,
            renotify_minutes: None,
            ..alert.definition.clone()
        },
        paused: None,
        paused_at: None,
        value: Some(1.0),
        evaluated_at: Some(at(300)),
        severity: Severity::Ok,
        severity_since: at(0),
        connectors: Vec::new(),
        starter: false,
        created_at: at(0),
        updated_at: at(0),
    };
    pin(
        "alerts",
        &AlertList {
            rules: vec![alert, paused, starter, retrieval],
            starters_withheld: vec![RuleMeasure::Stalled],
            starters_detail: Some(
                "This organization's plan holds 10 alerts across its projects, paused ones counted."
                    .into(),
            ),
        },
    );
}

/// The incidents lane: an alert's with the changes before it, a monitor's whose could not be read.
#[test]
fn incidents_answer_as_the_console_reads() {
    // Each two minutes earlier, so all came before the incident opened.
    let before_it: Vec<Change> = changes()
        .into_iter()
        .map(|change| Change {
            changed_at: change.changed_at - TimeDelta::minutes(2),
            ..change
        })
        .collect();
    pin(
        "incidents",
        &IncidentList {
            incidents: vec![
                IncidentAnswer {
                    row: incident(),
                    changes: Some(before_it),
                    changes_more: false,
                },
                IncidentAnswer {
                    row: IncidentRow {
                        id: Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2b2a),
                        rule_id: None,
                        rule_name: None,
                        monitor_id: Some(MONITOR),
                        agent: Some("nightly-triage".into()),
                        cause: Some(MonitorCause::Missed),
                        status: IncidentStatus::Open,
                        severity: Severity::Warning,
                        value: None,
                        acknowledged_at: None,
                        acknowledged_by: None,
                        reopened: 0,
                        traces: Vec::new(),
                        ..incident()
                    },
                    changes: None,
                    changes_more: false,
                },
            ],
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
        monitor_id: None,
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

/// The field inventory, two rows of it: one that names no person, and one
/// whose content can.
#[test]
fn the_field_inventory_answers_as_the_console_reads() {
    static MODEL: InventoryRow = InventoryRow {
        attribute: "gen_ai.request.model",
        on: ReadOn::ModelCall,
        kept: "model",
        needs: Needs::AnyMode,
        rule: "at most 256 characters",
    };
    static MESSAGES: InventoryRow = InventoryRow {
        attribute: "gen_ai.input.messages",
        on: ReadOn::ModelCall,
        kept: "sealed in Postgres alone",
        needs: Needs::Content,
        rule: "content",
    };
    pin(
        "field-inventory",
        &FieldInventory {
            conventions_commit: "a1b2c3d",
            rows: [&MODEL, &MESSAGES]
                .into_iter()
                .map(|row| Row {
                    row,
                    person: row.person(),
                })
                .collect(),
        },
    );
}

/// A project's privacy posture, one inventory row sent.
#[test]
fn the_privacy_posture_answers_as_the_console_reads() {
    pin("door-scrub", &DoorScrub::default());
    pin(
        "privacy",
        &Posture {
            door_scrub: Default::default(),
            days: 30,
            since: at(0),
            spans: 1_200,
            sent: vec![Sent {
                attribute: "gen_ai.input.messages".into(),
                spans: 400,
            }],
            retention: Retention {
                span_days: 90,
                max_span_days: 360,
                rollup_months: 13,
            },
        },
    );
}

/// The dashboards page's totals split by a tag from the spans: a named
/// value, a trace carrying none, and the rest folded.
#[test]
fn the_rollups_answer_as_the_console_reads() {
    use telmoni_telemetry::rollups::{Durations, MetricPoint, Metrics, TraceDurations};
    let point = |value: &str, others: bool, spend: f64| MetricPoint {
        at: at(0),
        value: Some(value.into()),
        others,
        spans: 12,
        failed: 1,
        traces: 2,
        failed_traces: 1,
        input_tokens: 4_000,
        output_tokens: 600,
        cache_read_input_tokens: 0,
        cache_write_input_tokens: 0,
        reasoning_output_tokens: 100,
        spend,
        duration_ms: Durations {
            avg: 420.0,
            p50: Some(300.0),
            p90: Some(900.0),
            p99: Some(1_400.0),
        },
        trace_duration_ms: TraceDurations {
            p50: Some(2_100.0),
            p95: Some(3_800.0),
            p99: None,
        },
    };
    pin(
        "rollups",
        &Metrics {
            days: 30,
            step: "day",
            group: Some(MetricsGroup::Tag),
            tag: Some("team".into()),
            points: vec![
                point("billing", false, 1.25),
                point("", false, 0.5),
                point("", true, 0.75),
            ],
            truncated: false,
        },
    );
}

/// How the organization divides its pool, as the Limits page reads it: the
/// plan's numbers, and a project capped beside one sharing the pool whole.
#[test]
fn the_limits_answer_as_the_console_reads() {
    use telmoni_telemetry::handler::limits::{Limits, PlanLimits, ProjectShare, Used};
    use telmoni_telemetry::project_limits::Caps;
    pin(
        "limits",
        &Limits {
            plan: PlanLimits {
                spans: Some(1_000_000),
                content_bytes: Some(1_000_000_000),
                alerts: Some(50),
                alert_deliveries_per_day: Some(2_000),
            },
            projects: vec![
                ProjectShare {
                    id: project().to_string(),
                    name: "Support bot".into(),
                    caps: Caps {
                        spans: Some(200_000),
                        alerts: Some(10),
                        alert_deliveries_per_day: Some(100),
                        ..Caps::default()
                    },
                    used: Used {
                        spans: Some(12_500),
                        content_bytes: 4_096,
                        alerts: 3,
                        monitors: 1,
                        objectives: 0,
                        alert_deliveries_today: Some(7),
                    },
                },
                ProjectShare {
                    id: "project_9tQwE2rYuI8oPaSd".into(),
                    name: "Research".into(),
                    caps: Caps::default(),
                    used: Used {
                        spans: None,
                        ..Used::default()
                    },
                },
            ],
        },
    );
}
