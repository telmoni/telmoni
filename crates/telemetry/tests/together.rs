//! Alerts read together: a pass reads every due alert of a project that
//! shares a kind and a window on the totals, or a measure and a window trace by
//! trace, in one query grouped by each alert's filters, and each alert's value,
//! severity, incident and notice is the one it reads alone.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, TimeDelta, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, NoticeLog, SECRET, clickhouse_or_skip};
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{
    MissingData, NotificationKind, OrganizationId, OrganizationRole, ProjectId, Role, Severity,
    SpanKind, SpanStatus,
};
use telmoni_telemetry::router;
use telmoni_telemetry::rules::db::{self as rules_db, RuleRow};
use telmoni_telemetry::rules::evaluate::{self, Together};
use telmoni_telemetry::rules::incidents;
use telmoni_telemetry::store::SpanRow;

const LANE: &str = "/internal/v1/telemetry/rules";

/// Alerts of every shape a pass reads: on the totals, of each kind, folded
/// each way, filtered by each field or none, read against the window before;
/// trace by trace, steps, repeats and stalls, filtered and not; and a cost, read
/// alone, among them.
fn mixed() -> Vec<Value> {
    let alert = |name: &str, kind: &str, measure: &str, aggregation: &str, threshold: f64| {
        json!({ "name": name, "kind": kind, "measure": measure, "aggregation": aggregation,
            "window": "hour", "operator": "gt", "alert_threshold": threshold })
    };
    let with = |mut alert: Value, field: &str, value: Value| {
        if let Some(fields) = alert.as_object_mut() {
            fields.insert(field.to_owned(), value);
        }
        alert
    };
    vec![
        alert("Traces", "agent", "count", "count", 3.0),
        alert("Share failed", "agent", "failures", "avg", 0.1),
        with(
            alert("Billing failed", "agent", "failures", "count", 0.0),
            "filters",
            json!({ "agent": ["billing-bot"] }),
        ),
        with(
            alert("Slow sonnet", "model_call", "duration", "p95", 500.0),
            "filters",
            json!({ "model": ["claude-sonnet-5-5"] }),
        ),
        with(
            alert("Calls in staging", "model_call", "duration", "p50", 500.0),
            "filters",
            json!({ "environment": ["staging"] }),
        ),
        alert("Call average", "model_call", "duration", "avg", 900.0),
        alert("Tokens", "model_call", "tokens", "sum", 1_000.0),
        with(
            alert("Searches", "tool_call", "count", "count", 0.0),
            "filters",
            json!({ "tool": ["search_docs"] }),
        ),
        with(
            alert("Docs index failing", "retrieval", "failures", "count", 0.0),
            "filters",
            json!({ "source": ["docs-index"] }),
        ),
        alert("Retrievals", "retrieval", "count", "count", 1.0),
        with(
            alert("Failed steps", "step", "count", "count", 0.0),
            "filters",
            json!({ "status": ["error"] }),
        ),
        with(
            alert("Traces against before", "agent", "count", "count", 50.0),
            "baseline",
            json!("previous"),
        ),
        alert("Steps", "agent", "steps", "max", 1.0),
        alert("Loops", "agent", "repeats", "max", 5.0),
        with(
            alert("Looper's loops", "agent", "repeats", "max", 5.0),
            "filters",
            json!({ "agent": ["looper"] }),
        ),
        with(
            alert("Stalls", "agent", "stalled", "count", 0.0),
            "operator",
            json!("gte"),
        ),
        with(
            with(
                alert("Support stalls", "agent", "stalled", "count", 1.0),
                "operator",
                json!("gte"),
            ),
            "filters",
            json!({ "agent": ["support-bot"] }),
        ),
        with(
            alert("Billing loops", "agent", "repeats", "max", 5.0),
            "filters",
            json!({ "agent": ["billing-bot"] }),
        ),
        with(
            with(
                alert("Billing stalls", "agent", "stalled", "count", 1.0),
                "operator",
                json!("gte"),
            ),
            "filters",
            json!({ "agent": ["billing-bot"] }),
        ),
        alert("Cost cap", "agent", "cost", "max", 0.01),
    ]
}

/// The spans the alerts read: the varied traces, a trace that loops on one tool,
/// one left open past the window before, and retrievals of two sources beside
/// a tool of one's name.
fn spans(project: &ProjectId) -> Vec<SpanRow> {
    let now = Utc::now();
    let mut rows = common::varied_traces(project);
    let searched = [
        (SpanKind::Retrieval, "docs-index", SpanStatus::Error),
        (SpanKind::Retrieval, "docs-index", SpanStatus::Ok),
        (SpanKind::Retrieval, "faq-index", SpanStatus::Error),
        (SpanKind::ToolCall, "docs-index", SpanStatus::Error),
    ];
    for (n, (kind, name, status)) in searched.into_iter().enumerate() {
        rows.push(SpanRow {
            span_id: format!("{:016x}", 0xb00 + n),
            status,
            ..common::received(project, kind, name, 100, now - TimeDelta::minutes(20))
        });
    }
    let looping = Uuid::new_v4();
    for n in 0..8_i64 {
        rows.push(SpanRow {
            trace_id: looping.simple().to_string(),
            agent: "looper".into(),
            span_id: format!("{:016x}", 0x900 + n),
            started_at: now - TimeDelta::minutes(30) + TimeDelta::seconds(n),
            ended_at: now - TimeDelta::minutes(30)
                + TimeDelta::seconds(n)
                + TimeDelta::milliseconds(100),
            ..common::received(
                project,
                SpanKind::ToolCall,
                "fetch_page",
                100,
                now - TimeDelta::minutes(29),
            )
        });
    }
    let stalled = Uuid::new_v4();
    for (n, kind) in [SpanKind::Step, SpanKind::ModelCall]
        .into_iter()
        .enumerate()
    {
        let at =
            now - TimeDelta::minutes(90) + TimeDelta::seconds(i64::try_from(n).expect("small"));
        rows.push(SpanRow {
            trace_id: stalled.simple().to_string(),
            agent: "support-bot".into(),
            span_id: format!("{:016x}", 0xa00 + n),
            status: SpanStatus::Ok,
            ..common::received(project, kind, "plan", 100, at)
        });
    }
    rows
}

async fn made(
    app: &axum::Router,
    organization: &OrganizationId,
    project: &ProjectId,
    body: &Value,
) {
    let request = Request::builder()
        .method("POST")
        .uri(LANE)
        .header(SERVICE_SECRET_HEADER, SECRET)
        .header("authorization", "Bearer tok_user_member")
        .header("x-organization-id", organization.as_str())
        .header("x-project-id", project.as_str())
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("a request");
    let response = app.clone().oneshot(request).await.expect("an answer");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
}

async fn rules_of(pool: &PgPool, project: &ProjectId) -> Vec<RuleRow> {
    let mut tx = project_scope(pool, project).await.expect("the scope");
    let rules = rules_db::rules(&mut tx, project).await.expect("the alerts");
    tx.commit().await.expect("commit");
    rules
}

/// ⚠ Read together, every alert of a fixture of mixed alerts reads what it
/// reads alone, the batch's groups read once each; and the pass that reads
/// them so settles each at the severity its own value sets, an incident
/// opened and told for each that breaches and for no other.
#[sqlx::test]
async fn alerts_read_together_read_what_each_reads_alone(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let auth = AuthStub::new();
    let organization = OrganizationId::new();
    let project = ProjectId::new();
    auth.seat(
        "user_member",
        &organization,
        &project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    let notices = NoticeLog::new();
    let state = common::state_with_console(&pool, auth, notices.clone());
    let app = router(Arc::clone(&state));
    ch.store
        .insert_spans(&spans(&project))
        .await
        .expect("the spans are kept");
    for alert in mixed() {
        made(&app, &organization, &project, &alert).await;
    }
    let rules = rules_of(&pool, &project).await;
    assert_eq!(rules.len(), mixed().len());

    let now = Utc::now();
    let refs: Vec<&RuleRow> = rules.iter().collect();
    let together = Together::read(&state, &refs, now).await;
    let mut alone = Vec::new();
    for rule in &rules {
        let by_itself = evaluate::value_of(&state, rule, now)
            .await
            .unwrap_or_else(|_| panic!("{} reads alone", rule.definition.name));
        let read = evaluate::value_with(&state, rule, now, &together)
            .await
            .unwrap_or_else(|_| panic!("{} reads together", rule.definition.name));
        assert_eq!(
            read, by_itself,
            "{} reads together what it reads alone",
            rule.definition.name
        );
        let batched = together.of(rule, now).is_some();
        assert_eq!(
            batched,
            rule.definition.measure != telmoni_shared::RuleMeasure::Cost,
            "{} is read together unless it is a cost",
            rule.definition.name
        );
        alone.push((rule.id, rule.definition.name.clone(), by_itself));
    }
    let breaching: Vec<&str> = alone
        .iter()
        .filter(|(_, _, value)| value.is_some_and(|value| value > 0.0))
        .map(|(_, name, _)| name.as_str())
        .collect();
    assert!(
        breaching.len() >= 8,
        "the fixture moves most of its alerts: {breaching:?} of {alone:?}"
    );

    evaluate::pass(&state).await.expect("a pass");
    let settled = rules_of(&pool, &project).await;
    for rule in &settled {
        let (_, name, value) = alone
            .iter()
            .find(|(id, ..)| *id == rule.id)
            .expect("the alert read alone");
        let value = value.or((rule.definition.missing_data == MissingData::Zero).then_some(0.0));
        assert_eq!(rule.value, value, "{name} settles at what it reads alone");
        let expected = incidents::next_severity(
            &RuleRow {
                severity: Severity::Unknown,
                ..rule.clone()
            },
            value,
        );
        assert_eq!(
            rule.severity, expected,
            "{name} at the severity its value sets"
        );
    }
    let opened = notices
        .notices()
        .into_iter()
        .filter(|notice| notice.kind == NotificationKind::IncidentOpened)
        .count();
    let breached = settled
        .iter()
        .filter(|rule| rule.severity.is_breach())
        .count();
    assert!(breached >= 8, "most of the fixture breaches: {breached}");
    assert_eq!(opened, breached, "one incident told for each that breached");
}

/// A group whose alerts read nothing at all reads each as none, as alone.
#[sqlx::test]
async fn alerts_over_an_empty_window_read_none_together_as_alone(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let auth = AuthStub::new();
    let organization = OrganizationId::new();
    let project = ProjectId::new();
    auth.seat(
        "user_member",
        &organization,
        &project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    let state = common::state_with_console(&pool, auth, NoticeLog::new());
    let app = router(Arc::clone(&state));
    for alert in mixed() {
        made(&app, &organization, &project, &alert).await;
    }
    let rules = rules_of(&pool, &project).await;
    let now: DateTime<Utc> = Utc::now();
    let refs: Vec<&RuleRow> = rules.iter().collect();
    let together = Together::read(&state, &refs, now).await;
    for rule in &rules {
        let by_itself = evaluate::value_of(&state, rule, now)
            .await
            .unwrap_or_else(|_| panic!("alone"));
        let read = evaluate::value_with(&state, rule, now, &together)
            .await
            .unwrap_or_else(|_| panic!("together"));
        assert_eq!(read, by_itself, "{}", rule.definition.name);
    }
}
