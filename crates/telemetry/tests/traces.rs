//! The trace lanes against a real ClickHouse: the list sums each run's spans
//! into one row, newest first, filtered by any of its columns and sorted by
//! any, and names only the project's own runs; a trace is its run's spans in
//! the order they started, and another project's run id is a 404 here; a
//! query a parameter does not take is a 400 that echoes none of it.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Duration, SubsecRound, Utc};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, SECRET, clickhouse_or_skip, span, state_with_stores};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role, SpanKind, SpanStatus};
use telmoni_telemetry::router;
use telmoni_telemetry::store::SpanRow;

const LANE: &str = "/internal/telemetry/traces";
const OWNER: &str = "user_owner";
const ADMIN: &str = "user_admin";
const MEMBER: &str = "user_member";
const STRANGER: &str = "user_stranger";

/// Two projects: ours, with its three people seated, and another's.
struct Projects {
    app: Router,
    organization: OrganizationId,
    ours: ProjectId,
    theirs: ProjectId,
}

fn projects(pool: &PgPool) -> Projects {
    let (organization, ours, theirs) = (OrganizationId::new(), ProjectId::new(), ProjectId::new());
    let auth = AuthStub::new();
    for (user, role, seat) in [
        (OWNER, OrganizationRole::Owner, None),
        (ADMIN, OrganizationRole::Admin, None),
        (MEMBER, OrganizationRole::Member, Some(Role::Member)),
    ] {
        auth.seat(user, &organization, &ours, role, seat);
    }
    auth.seat(
        "user_other",
        &OrganizationId::new(),
        &theirs,
        OrganizationRole::Owner,
        None,
    );
    Projects {
        app: router(state_with_stores(pool, auth)),
        organization,
        ours,
        theirs,
    }
}

impl Projects {
    fn request(&self, user: &str, project: &ProjectId, path: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(path)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", format!("Bearer tok_{user}"))
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", project.as_str())
            .body(Body::empty())
            .expect("a request")
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("the router answers");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("a JSON body")
        };
        (status, body)
    }

    /// The list as `MEMBER` reads it, with `query` (`"?agent=…"` or empty).
    async fn list(&self, query: &str) -> Value {
        let (status, body) = self
            .send(self.request(MEMBER, &self.ours, &format!("{LANE}{query}")))
            .await;
        assert_eq!(status, StatusCode::OK, "{query}: {body}");
        body
    }
}

/// The runs a list answers, in its order.
fn ids(list: &Value) -> Vec<Uuid> {
    list["traces"]
        .as_array()
        .expect("traces")
        .iter()
        .map(|t| {
            t["run_id"]
                .as_str()
                .expect("a run id")
                .parse()
                .expect("a UUID")
        })
        .collect()
}

/// The list's row for `run`.
fn trace(list: &Value, run: Uuid) -> &Value {
    list["traces"]
        .as_array()
        .expect("traces")
        .iter()
        .find(|t| t["run_id"] == run.to_string())
        .expect("the run is listed")
}

/// A finished span of `run`, started at `started` and lasting `millis`, its
/// id from `n`, received now.
fn span_at(
    project: &ProjectId,
    run: Uuid,
    kind: SpanKind,
    name: &str,
    started: DateTime<Utc>,
    millis: i64,
    n: u64,
) -> SpanRow {
    SpanRow {
        run_id: run,
        span_id: format!("{n:016x}"),
        started_at: started,
        ended_at: started + Duration::milliseconds(millis),
        ..span(project, kind, name, millis)
    }
}

/// Three runs of ours — one whose run span has not arrived — and one of
/// another project's, with one of each kind of span among them.
struct Seeded {
    a: Uuid,
    b: Uuid,
    c: Uuid,
    d: Uuid,
}

async fn seed(ch: &common::ClickHouse, p: &Projects) -> Seeded {
    let now = Utc::now().trunc_subsecs(6);
    let (a, b, c, d) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let a_start = now - Duration::minutes(10);
    let b_start = now - Duration::minutes(5);
    let c_start = now - Duration::minutes(1);
    let rows = vec![
        // Run A: a report, one user's, four spans, its tool call failed.
        SpanRow {
            user_id: "user-a".into(),
            conversation_id: "conv-1".into(),
            tags: vec![("team".into(), "search".into())],
            bound_max_steps: Some(40),
            sample_rate: Some(1.0),
            ..span_at(
                &p.ours,
                a,
                SpanKind::Run,
                "nightly-report",
                a_start,
                2_000,
                1,
            )
        },
        SpanRow {
            parent_span_id: format!("{:016x}", 1),
            user_id: "user-a".into(),
            conversation_id: "conv-1".into(),
            ..span_at(
                &p.ours,
                a,
                SpanKind::Step,
                "plan the reply",
                a_start + Duration::milliseconds(10),
                1_900,
                2,
            )
        },
        SpanRow {
            parent_span_id: format!("{:016x}", 2),
            model: "claude-sonnet-5-5".into(),
            provider: "anthropic".into(),
            input_tokens: 1_200,
            output_tokens: 300,
            cache_read_input_tokens: 100,
            cache_write_input_tokens: 50,
            user_id: "user-a".into(),
            conversation_id: "conv-1".into(),
            attributes: vec![("gen_ai.response.id".into(), "resp_01".into())],
            ..span_at(
                &p.ours,
                a,
                SpanKind::ModelCall,
                "chat claude-sonnet-5-5",
                a_start + Duration::milliseconds(20),
                1_500,
                3,
            )
        },
        SpanRow {
            parent_span_id: format!("{:016x}", 2),
            status: SpanStatus::Error,
            error_type: "TimeoutError".into(),
            user_id: "user-a".into(),
            conversation_id: "conv-1".into(),
            ..span_at(
                &p.ours,
                a,
                SpanKind::ToolCall,
                "search_docs",
                a_start + Duration::milliseconds(1_600),
                200,
                4,
            )
        },
        // Run B: a support bot's, ended in error, a cheaper model.
        SpanRow {
            agent: "support-bot".into(),
            status: SpanStatus::Error,
            error_type: "RuntimeError".into(),
            user_id: "user-b".into(),
            conversation_id: "conv-2".into(),
            ..span_at(&p.ours, b, SpanKind::Run, "support-bot", b_start, 500, 5)
        },
        SpanRow {
            agent: "support-bot".into(),
            parent_span_id: format!("{:016x}", 5),
            model: "gpt-x".into(),
            provider: "openai".into(),
            input_tokens: 10,
            output_tokens: 5,
            user_id: "user-b".into(),
            conversation_id: "conv-2".into(),
            ..span_at(
                &p.ours,
                b,
                SpanKind::ModelCall,
                "chat gpt-x",
                b_start + Duration::milliseconds(5),
                400,
                6,
            )
        },
        // Run C: still running — its model call arrived, its run span has not.
        SpanRow {
            agent: "zeta".into(),
            model: "gpt-x".into(),
            provider: "openai".into(),
            ..span_at(
                &p.ours,
                c,
                SpanKind::ModelCall,
                "chat gpt-x",
                c_start,
                100,
                7,
            )
        },
        // Run D: another project's.
        span_at(&p.theirs, d, SpanKind::Run, "nightly-report", now, 300, 8),
    ];
    ch.store.insert_spans(&rows).await.expect("the spans land");
    Seeded { a, b, c, d }
}

#[sqlx::test]
async fn the_list_sums_each_run_newest_first_and_names_only_the_projects_runs(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    let Seeded { a, b, c, d } = seed(&ch, &p).await;

    let list = p.list("").await;
    assert_eq!(ids(&list), [c, b, a], "newest first, and none of {d}");
    assert_eq!(list["more"], false);
    assert_eq!(list["days"], 7);
    assert_eq!(list["limit"], 100);
    assert_eq!(list["sort"], "started");
    assert_eq!(list["dir"], "desc");

    let report = trace(&list, a);
    assert_eq!(report["agent"], "nightly-report");
    assert_eq!(report["user_id"], "user-a");
    assert_eq!(report["conversation_id"], "conv-1");
    assert_eq!(report["ended"], true);
    assert_eq!(report["status"], "ok");
    assert_eq!(report["duration_ms"], 2_000);
    assert_eq!(report["spans"], 4);
    assert_eq!(report["steps"], 1);
    assert_eq!(report["model_calls"], 1);
    assert_eq!(report["tool_calls"], 1);
    assert_eq!(report["errors"], 1);
    assert_eq!(report["input_tokens"], 1_200);
    assert_eq!(report["output_tokens"], 300);
    assert_eq!(report["cache_read_input_tokens"], 100);
    assert_eq!(report["cache_write_input_tokens"], 50);
    assert_eq!(report["models"], serde_json::json!(["claude-sonnet-5-5"]));
    assert_eq!(report["tools"], serde_json::json!(["search_docs"]));

    let bot = trace(&list, b);
    assert_eq!(bot["status"], "error");
    assert_eq!(bot["ended"], true);
    assert_eq!(bot["duration_ms"], 500);

    // A run whose own span has not arrived is listed from what has, as
    // still running, its status from its spans.
    let running = trace(&list, c);
    assert_eq!(running["ended"], false);
    assert_eq!(running["status"], "ok");
    assert_eq!(running["agent"], "zeta");
    assert_eq!(running["user_id"], "");

    for user in [OWNER, ADMIN] {
        let (status, _) = p.send(p.request(user, &p.ours, LANE)).await;
        assert_eq!(status, StatusCode::OK, "{user}");
    }
    // Theirs, read by their own owner, is their one run and none of ours: the
    // row policy and the read's own WHERE each name the projects.
    let (status, body) = p.send(p.request("user_other", &p.theirs, LANE)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), [d]);
    let (status, _) = p.send(p.request(STRANGER, &p.ours, LANE)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn the_list_filters_by_any_column_and_sorts_by_any(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    let Seeded { a, b, c, .. } = seed(&ch, &p).await;

    assert_eq!(ids(&p.list("?agent=support-bot").await), [b]);
    assert_eq!(ids(&p.list("?user=user-a").await), [a]);
    assert_eq!(ids(&p.list("?conversation=conv-2").await), [b]);
    assert_eq!(ids(&p.list("?status=error").await), [b]);
    assert_eq!(ids(&p.list("?status=ok").await), [c, a]);
    assert_eq!(ids(&p.list("?model=gpt-x").await), [c, b]);
    assert_eq!(ids(&p.list("?tool=search_docs").await), [a]);
    assert!(ids(&p.list("?agent=nobody").await).is_empty());
    assert_eq!(ids(&p.list("?model=gpt-x&status=error").await), [b]);

    assert_eq!(ids(&p.list("?sort=tokens").await), [a, b, c], "most first");
    assert_eq!(ids(&p.list("?sort=tokens&dir=asc").await), [c, b, a]);
    assert_eq!(
        ids(&p.list("?sort=agent").await),
        [a, b, c],
        "the alphabet's order"
    );
    assert_eq!(ids(&p.list("?sort=agent&dir=desc").await), [c, b, a]);
    assert_eq!(ids(&p.list("?sort=duration").await), [a, b, c]);
    assert_eq!(
        ids(&p.list("?sort=steps").await)[0],
        a,
        "the one with a step first"
    );
    let by_user = p.list("?sort=user").await;
    assert_eq!(
        ids(&by_user),
        [c, a, b],
        "no user first, then the users in order"
    );
    assert_eq!(ids(&p.list("?sort=conversation&dir=desc").await), [b, a, c]);
    assert_eq!(ids(&p.list("?sort=status").await)[0], b, "error before ok");
    assert_eq!(ids(&p.list("?sort=started&dir=asc").await), [a, b, c]);

    let two = p.list("?limit=2").await;
    assert_eq!(ids(&two), [c, b]);
    assert_eq!(two["more"], true);
    assert_eq!(two["limit"], 2);
    let one_day = p.list("?days=1").await;
    assert_eq!(ids(&one_day).len(), 3);
    assert_eq!(one_day["days"], 1);
}

#[sqlx::test]
async fn a_trace_is_its_runs_spans_in_order_and_another_projects_run_is_not_here(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    let Seeded { a, d, .. } = seed(&ch, &p).await;

    let (status, body) = p
        .send(p.request(MEMBER, &p.ours, &format!("{LANE}/{a}")))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["run_id"], a.to_string());
    assert_eq!(body["truncated"], false);
    let spans = body["spans"].as_array().expect("spans");
    let kinds: Vec<&str> = spans
        .iter()
        .map(|s| s["kind"].as_str().expect("a kind"))
        .collect();
    assert_eq!(
        kinds,
        ["run", "step", "model_call", "tool_call"],
        "in the order they started"
    );
    let run = &spans[0];
    assert_eq!(run["name"], "nightly-report");
    assert_eq!(run["parent_span_id"], "");
    assert_eq!(run["tags"], serde_json::json!({ "team": "search" }));
    assert_eq!(run["bound_max_steps"], 40);
    assert_eq!(run["sample_rate"], 1.0);
    assert_eq!(run["user_id"], "user-a");
    let call = &spans[2];
    assert_eq!(call["parent_span_id"], format!("{:016x}", 2));
    assert_eq!(call["model"], "claude-sonnet-5-5");
    assert_eq!(call["input_tokens"], 1_200);
    assert_eq!(
        call["attributes"],
        serde_json::json!({ "gen_ai.response.id": "resp_01" })
    );
    let tool = &spans[3];
    assert_eq!(tool["status"], "error");
    assert_eq!(tool["error_type"], "TimeoutError");

    // Another project's run: not found here, as a run that never was.
    let (status, body) = p
        .send(p.request(MEMBER, &p.ours, &format!("{LANE}/{d}")))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, _) = p
        .send(p.request(MEMBER, &p.ours, &format!("{LANE}/{}", Uuid::new_v4())))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = p
        .send(p.request(MEMBER, &p.ours, &format!("{LANE}/not-a-run")))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, _) = p
        .send(p.request(STRANGER, &p.ours, &format!("{LANE}/{a}")))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// ⚠ A query a parameter does not take is refused before the store is asked,
/// naming the parameter and never the value, which a person typed.
#[sqlx::test]
async fn a_query_a_parameter_does_not_take_is_a_400_that_echoes_nothing(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.seat(
        MEMBER,
        &organization,
        &project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    // Nothing listens for ClickHouse here: a refusal never reaches it.
    let app = router(common::state(
        &pool,
        Arc::clone(&auth) as Arc<dyn telmoni_shared::seam::Auth>,
    ));
    for query in [
        "?days=0",
        "?days=361",
        "?limit=0",
        "?limit=501",
        "?status=maybe%3Cscript%3E",
        "?sort=spend",
        "?dir=sideways",
        "?nonsense=1",
    ] {
        let request = Request::builder()
            .method("GET")
            .uri(format!("{LANE}{query}"))
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", format!("Bearer tok_{MEMBER}"))
            .header("x-organization-id", organization.as_str())
            .header("x-project-id", project.as_str())
            .body(Body::empty())
            .expect("a request");
        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("the router answers");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(
            !body.contains("script") && !body.contains("sideways") && !body.contains("spend"),
            "{query} was echoed: {body}"
        );
    }

    // Without the service secret the lane is not there.
    let outside = Request::builder()
        .method("GET")
        .uri(LANE)
        .header("authorization", format!("Bearer tok_{MEMBER}"))
        .header("x-organization-id", organization.as_str())
        .header("x-project-id", project.as_str())
        .body(Body::empty())
        .expect("a request");
    let answered = app.oneshot(outside).await.expect("the router answers");
    assert_eq!(answered.status(), StatusCode::UNAUTHORIZED);
}
