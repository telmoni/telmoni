//! A run's tags changed in the console, through the module's own router: any
//! member of the project adds and removes them by key on the run's every
//! span, which every read then carries, each change one audit row naming
//! the keys and never a value; another project's member is told no such run,
//! and a change past the tags a span carries or the project's cardinality
//! for the day is refused, changing nothing.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, SubsecRound, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, SECRET, clickhouse_or_skip, span, state_with_stores};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role, SpanKind};
use telmoni_telemetry::ingest::cardinality;
use telmoni_telemetry::store::SpanRow;
use telmoni_telemetry::{AppState, contract, router};

const LANE: &str = "/internal/v1/telemetry/traces";
const MEMBER: &str = "user_member";
const STRANGER: &str = "user_other";

/// Our project, its member seated, and another organization's project.
struct Projects {
    app: Router,
    state: Arc<AppState>,
    organization: OrganizationId,
    ours: ProjectId,
    theirs: ProjectId,
}

fn projects(pool: &PgPool) -> Projects {
    let (organization, ours, theirs) = (OrganizationId::new(), ProjectId::new(), ProjectId::new());
    let auth = AuthStub::probing(pool);
    auth.seat(
        MEMBER,
        &organization,
        &ours,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    auth.seat(
        STRANGER,
        &OrganizationId::new(),
        &theirs,
        OrganizationRole::Owner,
        None,
    );
    let state = state_with_stores(pool, auth);
    Projects {
        app: router(state.clone()),
        state,
        organization,
        ours,
        theirs,
    }
}

impl Projects {
    async fn send(
        &self,
        method: &str,
        user: &str,
        project: &ProjectId,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", format!("Bearer tok_{user}"))
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", project.as_str());
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
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

    /// A change to `run`'s tags as our member sends it.
    async fn tag(&self, run: Uuid, change: Value) -> (StatusCode, Value) {
        let path = format!("{LANE}/{run}/tags");
        self.send("PATCH", MEMBER, &self.ours, &path, Some(change))
            .await
    }

    /// A trace lane as our member reads it.
    async fn get(&self, path: &str) -> Value {
        let (status, body) = self.send("GET", MEMBER, &self.ours, path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        body
    }

    /// The runs the list answers under `query`.
    async fn runs(&self, query: &str) -> Vec<String> {
        let list = self.get(&format!("{LANE}{query}")).await;
        list.get("traces")
            .and_then(Value::as_array)
            .expect("traces")
            .iter()
            .map(|t| t["run_id"].as_str().expect("a run id").to_owned())
            .collect()
    }

    /// Each span's tags of `run`, by span id.
    async fn span_tags(&self, run: Uuid) -> Vec<(String, Value)> {
        let trace = self.get(&format!("{LANE}/{run}")).await;
        let mut spans: Vec<(String, Value)> = trace
            .get("spans")
            .and_then(Value::as_array)
            .expect("spans")
            .iter()
            .map(|s| {
                (
                    s["span_id"].as_str().expect("a span id").to_owned(),
                    s["tags"].clone(),
                )
            })
            .collect();
        spans.sort_by(|a, b| a.0.cmp(&b.0));
        spans
    }

    /// The changes to traces in our organization's chain: resource and
    /// details.
    async fn changes(&self, pool: &PgPool) -> Vec<(String, Value)> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT resource_id, metadata::text FROM audit.events \
              WHERE organization_id = $1 AND action = 'updated' AND resource_kind = 'trace' \
              ORDER BY seq",
        )
        .bind(&self.organization)
        .fetch_all(pool)
        .await
        .expect("read the chain");
        rows.into_iter()
            .map(|(run, details)| (run, serde_json::from_str(&details).expect("JSON")))
            .collect()
    }
}

/// A run of ours: its own span carrying `tags`, and a model call carrying
/// one of its own, as an application's instrumentation may set.
async fn land(ch: &common::ClickHouse, project: &ProjectId, tags: &[(&str, &str)]) -> Uuid {
    land_in(ch, project, "", tags).await
}

/// [`land`], the run in `session`.
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
            run_id: run,
            span_id: "00000000000000a1".into(),
            started_at: started,
            session_id: session.into(),
            tags: owned(tags),
            ..span(project, SpanKind::Run, "nightly-report", 2_000)
        },
        SpanRow {
            run_id: run,
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

/// ⚠ A member adds and removes a run's tags by key; the run's every span
/// takes the change, keeping its own other tags, so the list's filter, its
/// facet and its download read it; each change is one audit row naming the
/// keys, never a value.
#[sqlx::test]
async fn a_runs_tags_are_changed_by_any_member_and_every_read_carries_them(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let p = projects(&pool);
    let run = land(&ch, &p.ours, &[("repo", "telmoni")]).await;
    let other = land(&ch, &p.ours, &[]).await;

    let (status, answer) = p
        .tag(
            run,
            json!({ "add": { "branch": "main-VALUE-NEVER-FILED", "repo": "core" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer,
        json!({ "run_id": run, "tags": { "branch": "main-VALUE-NEVER-FILED", "repo": "core" } })
    );
    assert_eq!(
        p.span_tags(run).await,
        [
            (
                "00000000000000a1".to_owned(),
                json!({ "branch": "main-VALUE-NEVER-FILED", "repo": "core" })
            ),
            (
                "00000000000000a2".to_owned(),
                json!({ "branch": "main-VALUE-NEVER-FILED", "repo": "core", "step": "fetch" })
            ),
        ],
        "every span takes the change and keeps its own other tags"
    );
    assert_eq!(
        p.runs("?tag=repo:core").await,
        [run.to_string()],
        "the filter reads it"
    );
    assert!(
        p.runs("?tag=repo:telmoni").await.is_empty(),
        "and not the value it replaced"
    );
    let facets = p.get(&format!("{LANE}/facets")).await;
    let values: Vec<&str> = facets["tag"]["values"]
        .as_array()
        .expect("values")
        .iter()
        .map(|v| v["value"].as_str().expect("a value"))
        .collect();
    assert!(
        values.contains(&"repo:core") && !values.contains(&"repo:telmoni"),
        "{values:?}"
    );
    let download = p.get(&format!("{LANE}/export?format=json")).await;
    assert!(
        download.to_string().contains("repo:core"),
        "the download reads it: {download}"
    );

    let (status, answer) = p.tag(run, json!({ "remove": ["branch", "absent"] })).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["tags"], json!({ "repo": "core" }));
    assert!(
        p.runs("?tag=branch:main-VALUE-NEVER-FILED")
            .await
            .is_empty()
    );
    assert_eq!(
        p.span_tags(other).await[0].1,
        json!({}),
        "another run untouched"
    );

    let changes = p.changes(&pool).await;
    assert_eq!(
        changes,
        [
            (
                run.to_string(),
                json!({ "added": ["branch", "repo"], "removed": [] })
            ),
            (
                run.to_string(),
                json!({ "added": [], "removed": ["absent", "branch"] })
            ),
        ]
    );
    assert!(!format!("{changes:?}").contains("VALUE-NEVER-FILED"));
}

/// Another project's member is told no such run; a change past the tags a
/// span carries, or the project's tag cardinality, is refused by its problem
/// type, and nothing is filed or changed.
#[sqlx::test]
async fn a_change_outside_the_project_or_past_a_cap_is_refused(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let p = projects(&pool);
    let run = land(&ch, &p.ours, &[("repo", "telmoni")]).await;
    let path = format!("{LANE}/{run}/tags");
    let add = json!({ "add": { "branch": "main" } });

    let (status, _) = p
        .send("PATCH", STRANGER, &p.theirs, &path, Some(add.clone()))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another project's member");
    let (status, _) = p.tag(Uuid::new_v4(), add.clone()).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a run the project does not hold"
    );

    for refused in [
        json!({}),
        json!({ "add": { "a:b": "x" } }),
        json!({ "add": { "": "x" } }),
        json!({ "add": { "k": "v".repeat(contract::TAG_VALUE_MAX + 1) } }),
        json!({ "add": { "repo": "x" }, "remove": ["repo"] }),
        json!({ "add": { "repo": "x" }, "colour": "red" }),
    ] {
        let (status, body) = p.tag(run, refused.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
    }

    let many: serde_json::Map<String, Value> = (0..contract::TAGS_MAX)
        .map(|n| (format!("k{n:02}"), json!("v")))
        .collect();
    let (status, body) = p.tag(run, json!({ "add": many })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["type"], "/errors/telemetry/tags-past-cap");
    assert_eq!(body["detail"], "a span carries up to 32 tags");

    if p.state.redis.connection().await.is_none() {
        eprintln!("skipping the cardinality half: Redis is not reachable");
    } else {
        let full: Vec<String> = (0..contract::TAG_VALUES_PER_DAY)
            .map(|n| format!("v{n}"))
            .collect();
        let taken: Vec<(&str, &str)> = full.iter().map(|v| ("branch", v.as_str())).collect();
        let admitted = cardinality::admit(&p.state.redis, &p.ours, Utc::now(), &taken).await;
        assert_eq!(admitted, None, "the day's values fill the key's cap");
        let (status, body) = p.tag(run, add.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["type"], "/errors/telemetry/tags-past-cap");
        assert_eq!(
            body["detail"],
            "a project's tag takes up to 500 values a UTC day"
        );
        let (status, _) = p.tag(run, json!({ "add": { "branch": "v7" } })).await;
        assert_eq!(status, StatusCode::OK, "a value taken that day is kept");
    }

    assert_eq!(
        p.span_tags(run).await[0].1["repo"],
        "telmoni",
        "nothing refused was written"
    );
    assert!(
        p.changes(&pool)
            .await
            .iter()
            .all(|(_, details)| details["added"] == json!(["branch"])),
        "nothing refused was filed"
    );
}

/// A sort by `tag:repo` orders the trace list by each run's value and the
/// sessions by each one's least among its runs, those without it last either
/// way; the list names the sort, a session's row its runs' tags, and the
/// users list, which no tag sorts, refuses it.
#[sqlx::test]
async fn a_sort_by_a_tag_orders_runs_and_sessions_those_without_it_last(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    let a = land_in(&ch, &p.ours, "s1", &[("repo", "b")]).await;
    let b = land_in(&ch, &p.ours, "s2", &[("repo", "a")]).await;
    let c = land_in(&ch, &p.ours, "s2", &[("repo", "c")]).await;
    let d = land_in(&ch, &p.ours, "s3", &[("team", "core")]).await;
    let ids = |runs: &[Uuid]| -> Vec<String> { runs.iter().map(Uuid::to_string).collect() };

    assert_eq!(p.runs("?sort=tag:repo").await, ids(&[b, a, c, d]));
    assert_eq!(p.runs("?sort=tag:repo&dir=desc").await, ids(&[c, a, b, d]));
    assert_eq!(
        p.get(&format!("{LANE}?sort=tag:repo")).await["sort"],
        "tag:repo"
    );

    let sessions = |list: &Value| -> Vec<String> {
        list.get("sessions")
            .and_then(Value::as_array)
            .expect("sessions")
            .iter()
            .map(|s| s["session_id"].as_str().expect("an id").to_owned())
            .collect()
    };
    let list = p.get("/internal/v1/telemetry/sessions?sort=tag:repo").await;
    assert_eq!(sessions(&list), ["s2", "s1", "s3"]);
    assert_eq!(
        list["sessions"][0]["tags"],
        json!(["repo:a", "repo:c", "step:fetch"])
    );
    let list = p
        .get("/internal/v1/telemetry/sessions?sort=tag:repo&dir=desc")
        .await;
    assert_eq!(sessions(&list), ["s1", "s2", "s3"]);

    for refused in [
        "/internal/v1/telemetry/users?sort=tag:repo",
        "/internal/v1/telemetry/traces?sort=tag:",
    ] {
        let (status, body) = p.send("GET", MEMBER, &p.ours, refused, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
    }
}
