//! The trace list sorted by a tag, through the module's own router: traces by
//! each one's value of the key, those without it last either way.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, SubsecRound, Utc};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, SECRET, clickhouse_or_skip, span, state_with_stores};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role, SpanKind};
use telmoni_telemetry::router;
use telmoni_telemetry::store::SpanRow;

const LANE: &str = "/internal/v1/telemetry/traces";
const MEMBER: &str = "user_member";

/// Our project, its member seated.
struct Projects {
    app: Router,
    organization: OrganizationId,
    ours: ProjectId,
}

fn projects(pool: &PgPool) -> Projects {
    let (organization, ours) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::probing(pool);
    auth.seat(
        MEMBER,
        &organization,
        &ours,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    Projects {
        app: router(state_with_stores(pool, auth)),
        organization,
        ours,
    }
}

impl Projects {
    /// A trace lane as our member reads it, whatever it answers.
    async fn send(&self, path: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("GET")
            .uri(path)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", format!("Bearer tok_{MEMBER}"))
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", self.ours.as_str())
            .body(Body::empty())
            .expect("a request");
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

    /// A trace lane as our member reads it.
    async fn get(&self, path: &str) -> Value {
        let (status, body) = self.send(path).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        body
    }

    /// The traces the list answers under `query`.
    async fn listed(&self, query: &str) -> Vec<String> {
        let list = self.get(&format!("{LANE}{query}")).await;
        list.get("traces")
            .and_then(Value::as_array)
            .expect("traces")
            .iter()
            .map(|t| t["trace_id"].as_str().expect("a trace id").to_owned())
            .collect()
    }
}

/// A trace of ours in `session`: its own span carrying `tags`, and a model call
/// carrying one of its own, as an application's instrumentation may set.
async fn land_in(
    ch: &common::ClickHouse,
    project: &ProjectId,
    session: &str,
    tags: &[(&str, &str)],
) -> Uuid {
    let run = Uuid::new_v4();
    let started = Utc::now().trunc_subsecs(6) - Duration::minutes(1);
    let owned = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    };
    let rows = vec![
        SpanRow {
            trace_id: run.simple().to_string(),
            span_id: "00000000000000a1".into(),
            started_at: started,
            session_id: session.into(),
            tags: owned(tags),
            ..span(project, SpanKind::Agent, "nightly-report", 2_000)
        },
        SpanRow {
            trace_id: run.simple().to_string(),
            span_id: "00000000000000a2".into(),
            parent_span_id: "00000000000000a1".into(),
            started_at: started,
            session_id: session.into(),
            tags: owned(&[("step", "fetch")]),
            ..span(project, SpanKind::ModelCall, "chat", 500)
        },
    ];
    ch.store.insert_spans(&rows).await.expect("the spans land");
    run
}

/// A sort by `tag:repo` orders the trace list by each trace's value, those
/// without it last either way; the list names the sort, and a key left out
/// is refused.
#[sqlx::test]
async fn a_sort_by_a_tag_orders_traces_those_without_it_last(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    let a = land_in(&ch, &p.ours, "s1", &[("repo", "b")]).await;
    let b = land_in(&ch, &p.ours, "s2", &[("repo", "a")]).await;
    let c = land_in(&ch, &p.ours, "s2", &[("repo", "c")]).await;
    let d = land_in(&ch, &p.ours, "s3", &[("team", "core")]).await;
    let ids = |traces: &[Uuid]| -> Vec<String> {
        traces
            .iter()
            .map(|trace| trace.simple().to_string())
            .collect()
    };

    assert_eq!(p.listed("?sort=tag:repo").await, ids(&[b, a, c, d]));
    assert_eq!(
        p.listed("?sort=tag:repo&dir=desc").await,
        ids(&[c, a, b, d])
    );
    assert_eq!(
        p.get(&format!("{LANE}?sort=tag:repo")).await["sort"],
        "tag:repo"
    );

    let refused = "/internal/v1/telemetry/traces?sort=tag:";
    let (status, body) = p.send(refused).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
}
