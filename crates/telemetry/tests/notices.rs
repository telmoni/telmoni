//! What an alert's notice carries, through the module's router and its loop:
//! the numbers that crossed it, the run that shows it — its last step, its
//! error, its steps, its tokens and what it spent, linked — and the traces
//! the alert read; a recovery the traces alone.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{TimeDelta, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, LoggedNotice, NoticeLog, SECRET, clickhouse_or_skip};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::{
    NotificationKind, OrganizationId, OrganizationRole, ProjectId, Role, SpanKind, SpanStatus,
};
use telmoni_telemetry::rules::evaluate;
use telmoni_telemetry::store::SpanRow;
use telmoni_telemetry::{AppState, router};

const RULES: &str = "/internal/v1/telemetry/rules";
const INCIDENTS: &str = "/internal/v1/telemetry/incidents";

struct Project {
    app: axum::Router,
    state: Arc<AppState>,
    notices: Arc<NoticeLog>,
    organization: OrganizationId,
    project: ProjectId,
}

impl Project {
    /// A project whose notices link the console, holding the spans `spans`
    /// makes for it.
    async fn new(
        pool: &PgPool,
        ch: &common::ClickHouse,
        spans: impl FnOnce(&ProjectId) -> Vec<SpanRow>,
    ) -> Self {
        let auth = AuthStub::new();
        let (organization, project) = (OrganizationId::new(), ProjectId::new());
        auth.seat(
            "user_member",
            &organization,
            &project,
            OrganizationRole::Member,
            Some(Role::Member),
        );
        let notices = NoticeLog::new();
        let state = common::state_with_console(pool, auth, notices.clone());
        ch.store
            .insert_spans(&spans(&project))
            .await
            .expect("the spans are kept");
        Self {
            app: router(state.clone()),
            state,
            notices,
            organization,
            project,
        }
    }

    async fn send(&self, method: &str, uri: &str, body: &Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", "Bearer tok_user_member")
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", self.project.as_str())
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("a request");
        let response = self.app.clone().oneshot(request).await.expect("an answer");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// The alert `body` makes, through its lane: its path.
    async fn alert(&self, body: &Value) -> String {
        let (status, made) = self.send("POST", RULES, body).await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
        format!(
            "{RULES}/{}",
            made.get("id").and_then(Value::as_str).expect("an id")
        )
    }

    async fn evaluate(&self, pool: &PgPool) {
        sqlx::query("UPDATE telemetry.rules SET evaluated_at = NULL")
            .execute(pool)
            .await
            .expect("due again");
        evaluate::pass(&self.state).await.expect("a pass");
    }

    /// The project's first incident, as its list answers it.
    async fn incident(&self) -> Value {
        let (status, listed) = self.send("GET", INCIDENTS, &Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        listed
            .pointer("/incidents/0")
            .cloned()
            .expect("an incident")
    }

    /// Where the console's pages for this project start.
    fn console(&self) -> String {
        format!(
            "https://console.example/{}/{}",
            self.organization, self.project
        )
    }
}

/// A span of `run`, ended `ago` seconds since after `millis` of work.
fn span(
    project: &ProjectId,
    run: Uuid,
    kind: SpanKind,
    name: &str,
    millis: i64,
    ago: i64,
) -> SpanRow {
    SpanRow {
        run_id: run,
        ..common::received(
            project,
            kind,
            name,
            millis,
            Utc::now() - TimeDelta::seconds(ago),
        )
    }
}

/// A notice's lines.
fn lines(notice: &LoggedNotice) -> Vec<&str> {
    notice.body.lines().collect()
}

/// ⚠ A breach names the run that shows it — the latest failed for a
/// failures alert, with its last step, its error, its steps and its tokens —
/// and links it and the traces the alert read; a reminder and the recovery
/// carry the traces alone.
#[sqlx::test]
async fn a_breach_names_the_run_that_shows_it_and_links_the_traces(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (failed, fine) = (Uuid::new_v4(), Uuid::new_v4());
    let project = Project::new(&pool, &ch, |project| {
        vec![
            span(project, failed, SpanKind::Step, "plan", 100, 30),
            span(project, failed, SpanKind::Step, "search", 100, 20),
            SpanRow {
                input_tokens: 1_000,
                output_tokens: 500,
                ..span(project, failed, SpanKind::ModelCall, "chat", 900, 25)
            },
            SpanRow {
                status: SpanStatus::Error,
                error_type: "TimeoutError".into(),
                ..span(project, failed, SpanKind::Run, "trip", 3_000, 10)
            },
            span(project, fine, SpanKind::Run, "trip", 2_000, 5),
        ]
    })
    .await;
    let calls = |threshold: f64| {
        json!({
            "name": "Failing trips", "kind": "run", "measure": "failures",
            "aggregation": "avg", "window": "hour", "operator": "gt",
            "alert_threshold": threshold, "renotify_minutes": 1,
        })
    };
    let rule = project.alert(&calls(0.1)).await;
    project.evaluate(&pool).await;

    let console = project.console();
    let told = project.notices.notices();
    let opened = told.first().expect("the breach told");
    assert_eq!(opened.kind, NotificationKind::IncidentOpened);
    let expected = [
        "The share failed of runs over the last hour is 50%, past the alert threshold of 10%."
            .to_owned(),
        format!(
            "The latest failed run: last step search, TimeoutError, 2 steps, 1500 tokens and \
             $0.00, no price for a call naming no model. {console}/traces/{failed}"
        ),
        format!("Traces: {console}/traces?days=1&status=error"),
    ];
    assert_eq!(
        lines(opened).get(..3),
        Some(&expected.each_ref().map(String::as_str)[..]),
        "{}",
        opened.body
    );
    assert_eq!(opened.metadata.pointer("/run/run_id"), Some(&json!(failed)));
    assert_eq!(
        opened.metadata.pointer("/run/error_type"),
        Some(&json!("TimeoutError"))
    );

    sqlx::query("UPDATE telemetry.rules SET told_at = now() - interval '2 minutes'")
        .execute(&pool)
        .await
        .expect("a reminder due");
    project.evaluate(&pool).await;
    let told = project.notices.notices();
    let reminded = told.get(1).expect("the reminder told");
    assert!(reminded.body.starts_with("Still: "), "{}", reminded.body);
    assert!(
        !reminded.body.contains("/traces/"),
        "a reminder names no run: {}",
        reminded.body
    );

    project.send("PUT", &rule, &calls(0.9)).await;
    project.evaluate(&pool).await;
    let told = project.notices.notices();
    let resolved = told.get(2).expect("the recovery told");
    assert_eq!(resolved.kind, NotificationKind::IncidentResolved);
    let said = lines(resolved);
    assert_eq!(
        said.get(1).copied(),
        Some(format!("Traces: {console}/traces?days=1&status=error").as_str())
    );
    assert!(
        !said.iter().any(|line| line.contains("/traces/")),
        "a recovery names no run: {}",
        resolved.body
    );
    assert_eq!(resolved.metadata.get("run"), Some(&Value::Null));
}

/// ⚠ An incident keeps the connectors that heard its notices and the runs they showed.
#[sqlx::test]
async fn an_incident_keeps_where_its_notices_went_and_the_runs_they_showed(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (failed, fine) = (Uuid::new_v4(), Uuid::new_v4());
    let project = Project::new(&pool, &ch, |project| {
        vec![
            SpanRow {
                status: SpanStatus::Error,
                error_type: "TimeoutError".into(),
                ..span(project, failed, SpanKind::Run, "trip", 3_000, 10)
            },
            span(project, fine, SpanKind::Run, "trip", 2_000, 5),
        ]
    })
    .await;
    let (pager, chat) = (Uuid::new_v4(), Uuid::new_v4());
    let failing = |alert: f64| {
        json!({
            "name": "Failing trips", "kind": "run", "measure": "failures",
            "aggregation": "avg", "window": "hour", "operator": "gt",
            "alert_threshold": alert, "warning_threshold": 0.1,
            "connectors": [
                { "connection_id": pager, "level": "alert" },
                { "connection_id": chat },
            ],
        })
    };

    // Half the trips failed: past the warning, short of the alert.
    let rule = project.alert(&failing(0.9)).await;
    project.evaluate(&pool).await;
    let opened = project.incident().await;
    assert_eq!(opened["severity"], "warning", "{opened}");
    assert_eq!(
        opened["told_to"],
        json!([chat]),
        "a warning reaches the connector hearing warnings alone"
    );
    assert_eq!(opened["runs"], json!([failed]));

    project.send("PUT", &rule, &failing(0.4)).await;
    project.evaluate(&pool).await;
    let risen = project.incident().await;
    assert_eq!(risen["severity"], "alert", "{risen}");
    let mut both = vec![pager, chat];
    both.sort_unstable();
    assert_eq!(risen["told_to"], json!(both), "the rise reached both");
    assert_eq!(
        risen["runs"],
        json!([failed]),
        "a run shown again is kept once"
    );

    let (status, carrying) = project
        .send("GET", &format!("{INCIDENTS}?run={failed}"), &Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK, "{carrying}");
    assert_eq!(
        carrying["incidents"].as_array().map(|listed| listed
            .iter()
            .map(|one| one["id"].clone())
            .collect::<Vec<_>>()),
        Some(vec![risen["id"].clone()])
    );
    let (_, none) = project
        .send("GET", &format!("{INCIDENTS}?run={fine}"), &Value::Null)
        .await;
    assert_eq!(none["incidents"], json!([]), "no notice showed that run");
    let (status, _) = project
        .send("GET", &format!("{INCIDENTS}?run=trip"), &Value::Null)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a run is named by its id");
}

/// ⚠ A notice that reaches nobody keeps no run, as nobody was shown it.
#[sqlx::test]
async fn a_run_is_kept_where_its_notice_reached_someone(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let first = Uuid::new_v4();
    let failed = |project: &ProjectId, run: Uuid, ago: i64| SpanRow {
        status: SpanStatus::Error,
        error_type: "TimeoutError".into(),
        ..span(project, run, SpanKind::Run, "trip", 1_000, ago)
    };
    let project = Project::new(&pool, &ch, |project| vec![failed(project, first, 30)]).await;
    let failing = |threshold: f64| {
        json!({
            "name": "Failing trips", "kind": "run", "measure": "failures",
            "aggregation": "count", "window": "hour", "operator": "gt",
            "alert_threshold": threshold,
        })
    };
    let rule = project.alert(&failing(0.0)).await;
    project.evaluate(&pool).await;
    assert_eq!(project.incident().await["runs"], json!([first]));

    project.send("PUT", &rule, &failing(5.0)).await;
    project.evaluate(&pool).await;
    assert_eq!(project.incident().await["status"], "resolved");
    let second = Uuid::new_v4();
    ch.store
        .insert_spans(&[failed(&project.project, second, 5)])
        .await
        .expect("the span is kept");
    project.send("PUT", &rule, &failing(0.0)).await;
    project.evaluate(&pool).await;
    let reopened = project.incident().await;
    assert_eq!(reopened["reopened"], 1, "{reopened}");
    assert_eq!(
        reopened["runs"],
        json!([first]),
        "the reopening, told to no connector, showed its run to nobody"
    );
}

/// A member's resolution is kept as where it went, to a connector linked since it opened.
#[sqlx::test]
async fn a_resolution_by_hand_is_kept_as_where_it_went(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let run = Uuid::new_v4();
    let project = Project::new(&pool, &ch, |project| {
        vec![SpanRow {
            status: SpanStatus::Error,
            ..span(project, run, SpanKind::Run, "trip", 1_000, 10)
        }]
    })
    .await;
    let linked = Uuid::new_v4();
    let failing = |connectors: Value| {
        json!({
            "name": "Failing trips", "kind": "run", "measure": "failures",
            "aggregation": "count", "window": "hour", "operator": "gt",
            "alert_threshold": 0.0, "connectors": connectors,
        })
    };
    let rule = project.alert(&failing(json!([]))).await;
    project.evaluate(&pool).await;
    let opened = project.incident().await;
    assert_eq!(opened["told_to"], json!([]), "the feed alone");

    project
        .send("PUT", &rule, &failing(json!([{ "connection_id": linked }])))
        .await;
    let id = opened["id"].as_str().expect("an id");
    let (status, resolved) = project
        .send("POST", &format!("{INCIDENTS}/{id}/resolve"), &Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(project.incident().await["told_to"], json!([linked]));
}

/// ⚠ A member's resolution of a reopened incident reaches its connectors alone.
#[sqlx::test]
async fn a_reopened_incident_resolved_by_hand_is_told_to_its_connectors(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let run = Uuid::new_v4();
    let project = Project::new(&pool, &ch, |project| {
        vec![SpanRow {
            status: SpanStatus::Error,
            ..span(project, run, SpanKind::Run, "trip", 1_000, 10)
        }]
    })
    .await;
    let linked = Uuid::new_v4();
    let failing = |threshold: f64| {
        json!({
            "name": "Failing trips", "kind": "run", "measure": "failures",
            "aggregation": "count", "window": "hour", "operator": "gt",
            "alert_threshold": threshold, "connectors": [{ "connection_id": linked }],
        })
    };
    let rule = project.alert(&failing(0.0)).await;
    project.evaluate(&pool).await;
    project.send("PUT", &rule, &failing(5.0)).await;
    project.evaluate(&pool).await;
    project.send("PUT", &rule, &failing(0.0)).await;
    project.evaluate(&pool).await;
    let reopened = project.incident().await;
    assert_eq!(reopened["reopened"], 1, "{reopened}");

    let id = reopened["id"].as_str().expect("an id");
    let (status, resolved) = project
        .send("POST", &format!("{INCIDENTS}/{id}/resolve"), &Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    let told = project.notices.notices();
    let last = told.last().expect("the resolution told");
    assert_eq!(
        (last.kind, last.reminded, last.connections.as_slice()),
        (
            NotificationKind::IncidentResolved,
            true,
            [linked].as_slice()
        ),
        "to the connector alone, the feed holding a resolution already"
    );
    assert!(
        last.body.starts_with("A member of the project resolved"),
        "{}",
        last.body
    );
}

/// ⚠ A latency breach names the run of its slowest call, one still going
/// with its tokens so far, and the traces link keeps the filters the list
/// has.
#[sqlx::test]
async fn a_latency_breach_names_the_run_of_its_slowest_call(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (quick, slow) = (Uuid::new_v4(), Uuid::new_v4());
    let call = |project: &ProjectId, run: Uuid, millis: i64, ago: i64| SpanRow {
        model: "claude-sonnet-5-5".into(),
        provider: "anthropic".into(),
        input_tokens: 40,
        output_tokens: 2,
        ..span(project, run, SpanKind::ModelCall, "chat", millis, ago)
    };
    let project = Project::new(&pool, &ch, |project| {
        vec![
            call(project, quick, 300, 20),
            call(project, slow, 4_000, 40),
        ]
    })
    .await;
    project
        .alert(&json!({
            "name": "Slow calls", "kind": "model_call", "measure": "duration",
            "aggregation": "p95", "window": "day", "operator": "gt", "alert_threshold": 1000.0,
            "filters": { "model": ["claude-sonnet-5-5"], "provider": ["anthropic"] },
        }))
        .await;
    project.evaluate(&pool).await;

    let console = project.console();
    let told = project.notices.notices();
    let opened = told.first().expect("the breach told");
    let said = lines(opened);
    assert_eq!(
        said.get(1).copied(),
        Some(
            format!(
                "The run of the slowest model call: 0 steps, 42 tokens and $0.00 so far, \
                 no price for claude-sonnet-5-5 (anthropic). {console}/traces/{slow}"
            )
            .as_str()
        ),
        "{}",
        opened.body
    );
    assert_eq!(
        said.get(2).copied(),
        Some(format!("Traces: {console}/traces?days=1&model=claude-sonnet-5-5").as_str())
    );
}

/// ⚠ The cost cap pages a run while it is going: its calls' costs summed as
/// they land, reported else priced, its spend in the notice and the models
/// no price covers named.
#[sqlx::test]
async fn a_run_past_its_cost_cap_pages_while_it_runs(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    sqlx::query(
        "INSERT INTO telemetry.model_prices (provider, model, input, output, cache_read, cache_write)
         VALUES ('openai', 'gpt-4o', 2.5e-6, 1e-5, 1.25e-6, 2.5e-6)",
    )
    .execute(&pool)
    .await
    .expect("a price");
    let (dear, cheap) = (Uuid::new_v4(), Uuid::new_v4());
    let call = |project: &ProjectId, run: Uuid, model: &str, ago: i64| SpanRow {
        model: model.into(),
        provider: "openai".into(),
        ..span(project, run, SpanKind::ModelCall, "chat", 800, ago)
    };
    let project = Project::new(&pool, &ch, |project| {
        vec![
            span(project, dear, SpanKind::Step, "research", 100, 40),
            SpanRow {
                cost: Some(3.0),
                ..call(project, dear, "gpt-4o", 35)
            },
            SpanRow {
                input_tokens: 100_000,
                output_tokens: 20_000,
                ..call(project, dear, "gpt-4o", 30)
            },
            SpanRow {
                input_tokens: 500,
                ..call(project, dear, "ft:gpt-4o:acme", 25)
            },
            SpanRow {
                cost: Some(0.4),
                ..call(project, cheap, "gpt-4o", 20)
            },
        ]
    })
    .await;
    project
        .alert(&json!({
            "name": "Cost cap", "kind": "run", "measure": "cost", "aggregation": "max",
            "window": "day", "operator": "gt", "alert_threshold": 3.0, "warning_threshold": 1.0,
        }))
        .await;
    project.evaluate(&pool).await;

    let console = project.console();
    let told = project.notices.notices();
    let opened = told.first().expect("the cap's breach told");
    assert_eq!(opened.kind, NotificationKind::IncidentOpened);
    assert_eq!(opened.metadata.get("severity"), Some(&json!("alert")));
    let said = lines(opened);
    assert_eq!(
        said.first().copied(),
        Some("The most a run spent over the last day is $3.45, past the alert threshold of $3.00."),
        "{}",
        opened.body
    );
    assert_eq!(
        said.get(1).copied(),
        Some(
            format!(
                "The run that spent the most: last step research, 1 step, 120500 tokens and \
                 $3.45 so far, no price for ft:gpt-4o:acme (openai). {console}/traces/{dear}"
            )
            .as_str()
        ),
        "the run has not ended, and the cap read it as it went"
    );
    let spent = opened
        .metadata
        .pointer("/run/spent")
        .and_then(Value::as_f64)
        .expect("the run's spend");
    assert!((spent - 3.45).abs() < 1e-9, "{spent}");
    assert_eq!(opened.metadata.pointer("/run/unpriced"), Some(&json!(1)));
}

/// ⚠ A notice keeps to its bounds whatever its alert's filters and its run's names.
#[sqlx::test]
async fn a_notice_keeps_to_its_bounds_past_the_longest_filters_and_names(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let long = |seed: char, chars: usize| std::iter::repeat_n(seed, chars).collect::<String>();
    let run = Uuid::new_v4();
    let project = Project::new(&pool, &ch, |project| {
        let mut spans = vec![
            span(project, run, SpanKind::Step, &long('𝔰', 145), 100, 30),
            SpanRow {
                status: SpanStatus::Error,
                error_type: long('𝔢', 145),
                environment: "production".into(),
                release: "v1".into(),
                ..span(project, run, SpanKind::Run, "trip", 3_000, 10)
            },
        ];
        spans.extend((0..5).map(|n| SpanRow {
            model: format!("{n}{}", long('𝔪', 127)),
            provider: long('𝔭', 128),
            input_tokens: 100,
            ..span(project, run, SpanKind::ModelCall, "chat", 100, 20)
        }));
        spans
    })
    .await;
    let fifty = |kept: &str| {
        let mut values: Vec<String> = (0..49)
            .map(|n| format!("{n:03}{}", long('a', 125)))
            .collect();
        values.push(kept.to_owned());
        values
    };
    project
        .alert(&json!({
            "name": long('n', 60), "kind": "run", "measure": "failures", "aggregation": "avg",
            "window": "hour", "operator": "gt", "alert_threshold": 0.1,
            "filters": {
                "agent": fifty("nightly-report"),
                "environment": fifty("production"),
                "release": fifty("v1"),
            },
        }))
        .await;
    project.evaluate(&pool).await;

    let told = project.notices.notices();
    let opened = told.first().expect("the breach told");
    assert!(
        !opened.body.contains("Traces: "),
        "a link past its bounds is left out: {}",
        opened.body
    );
    assert!(
        opened.body.contains(&format!("/traces/{run}")),
        "{}",
        opened.body
    );
    let step = opened
        .metadata
        .pointer("/run/last_step")
        .and_then(Value::as_str)
        .expect("the run's last step");
    assert_eq!(step.chars().count(), 120, "{step}");
    assert_eq!(opened.metadata.pointer("/run/unpriced"), Some(&json!(5)));
}
