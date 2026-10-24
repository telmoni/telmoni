//! The trace lanes against a real ClickHouse: the list sums each run's spans
//! into one row, newest first, filtered by any of its columns — by several
//! values of one — sorted by any, paged and counted, and names only the
//! project's own runs; the facets count each value under the other filters,
//! the histogram each run in the bar it started in, and a download is the
//! list's rows as a file; a trace is its run's spans in the order they
//! started, and another project's run id is a 404 here; a query a parameter
//! does not take is a 400 that echoes none of it.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::collections::BTreeMap;
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
use telmoni_telemetry::traces::{RUNS_MAX, TRACE_UNITS_MAX};

const LANE: &str = "/internal/v1/telemetry/traces";
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
        self.get(&format!("{LANE}{query}")).await
    }

    /// Any of the trace lanes as `MEMBER` reads it, `path` with its query.
    async fn get(&self, path: &str) -> Value {
        let (status, body) = self.send(self.request(MEMBER, &self.ours, path)).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        body
    }

    /// A download as `user` asks for it: its status, its content type and
    /// its bytes.
    async fn download(&self, user: &str, query: &str) -> (StatusCode, String, Vec<u8>) {
        let response = self
            .app
            .clone()
            .oneshot(self.request(user, &self.ours, &format!("{LANE}/export{query}")))
            .await
            .expect("the router answers");
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        (status, content_type, bytes.to_vec())
    }
}

/// A facet's values and their counts, in the answer's order.
fn values(facets: &Value, facet: &str) -> Vec<(String, u64)> {
    facets
        .get(facet)
        .and_then(|f| f.get("values"))
        .and_then(Value::as_array)
        .expect("values")
        .iter()
        .map(|v| {
            (
                v["value"].as_str().expect("a value").to_owned(),
                v["runs"].as_u64().expect("a count"),
            )
        })
        .collect()
}

/// `pairs` as [`values`] answers them.
fn owned(pairs: &[(&str, u64)]) -> Vec<(String, u64)> {
    pairs.iter().map(|(v, n)| ((*v).to_owned(), *n)).collect()
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
/// another project's, with one of each kind of span among them; A sent from
/// production, B from staging, C from no deployment named.
struct Seeded {
    a: Uuid,
    b: Uuid,
    c: Uuid,
    d: Uuid,
}

/// `row` as sent from `environment` at `release`.
fn deployed(row: SpanRow, environment: &str, release: &str) -> SpanRow {
    SpanRow {
        environment: environment.into(),
        release: release.into(),
        ..row
    }
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
        deployed(
            SpanRow {
                user_id: "user-a".into(),
                session_id: "conv-1".into(),
                tags: vec![("team".into(), "search".into())],
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
            "production",
            "2026.10.17",
        ),
        deployed(
            SpanRow {
                parent_span_id: format!("{:016x}", 1),
                user_id: "user-a".into(),
                session_id: "conv-1".into(),
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
            "production",
            "2026.10.17",
        ),
        deployed(
            SpanRow {
                parent_span_id: format!("{:016x}", 2),
                model: "claude-sonnet-5-5".into(),
                provider: "anthropic".into(),
                input_tokens: 1_200,
                output_tokens: 300,
                cache_read_input_tokens: 100,
                cache_write_input_tokens: 50,
                user_id: "user-a".into(),
                session_id: "conv-1".into(),
                attributes: vec![
                    ("gen_ai.response.id".into(), "resp_01".into()),
                    ("gen_ai.prompt.name".into(), "triage-v2".into()),
                ],
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
            "production",
            "2026.10.17",
        ),
        deployed(
            SpanRow {
                parent_span_id: format!("{:016x}", 2),
                status: SpanStatus::Error,
                error_type: "TimeoutError".into(),
                user_id: "user-a".into(),
                session_id: "conv-1".into(),
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
            "production",
            "2026.10.17",
        ),
        // Run B: a support bot's, ended in error, a cheaper model, from
        // staging, its call tagged and prompted.
        deployed(
            SpanRow {
                agent: "support-bot".into(),
                status: SpanStatus::Error,
                error_type: "RuntimeError".into(),
                user_id: "user-b".into(),
                session_id: "conv-2".into(),
                ..span_at(&p.ours, b, SpanKind::Run, "support-bot", b_start, 500, 5)
            },
            "staging",
            "2026.10.18",
        ),
        deployed(
            SpanRow {
                agent: "support-bot".into(),
                parent_span_id: format!("{:016x}", 5),
                model: "gpt-x".into(),
                provider: "openai".into(),
                input_tokens: 10,
                output_tokens: 5,
                user_id: "user-b".into(),
                session_id: "conv-2".into(),
                tags: vec![("team".into(), "support".into())],
                attributes: vec![("gen_ai.prompt.name".into(), "triage-v2".into())],
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
            "staging",
            "2026.10.18",
        ),
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
    assert_eq!(list["total"], 3);
    assert_eq!(list["page"], 1);
    assert_eq!(list["days"], 7);
    assert_eq!(list["limit"], 100);
    assert_eq!(list["sort"], "started");
    assert_eq!(list["dir"], "desc");

    let report = trace(&list, a);
    assert_eq!(report["agent"], "nightly-report");
    assert_eq!(report["user_id"], "user-a");
    assert_eq!(report["session_id"], "conv-1");
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
    assert_eq!(report["environments"], serde_json::json!(["production"]));
    assert_eq!(report["releases"], serde_json::json!(["2026.10.17"]));

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
    assert_eq!(running["environments"], serde_json::json!([]));
    assert_eq!(running["releases"], serde_json::json!([]));

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
    assert_eq!(ids(&p.list("?session=conv-2").await), [b]);
    assert_eq!(ids(&p.list("?status=error").await), [b]);
    assert_eq!(ids(&p.list("?status=ok").await), [c, a]);
    assert_eq!(ids(&p.list("?model=gpt-x").await), [c, b]);
    assert_eq!(ids(&p.list("?tool=search_docs").await), [a]);
    assert!(ids(&p.list("?agent=nobody").await).is_empty());
    assert_eq!(ids(&p.list("?model=gpt-x&status=error").await), [b]);
    // The deployment, a tag and a prompt: a run carries each where any of
    // its spans does, the tag as key:value.
    assert_eq!(ids(&p.list("?environment=production").await), [a]);
    assert_eq!(
        ids(&p.list("?environment=staging&environment=production").await),
        [b, a]
    );
    assert_eq!(ids(&p.list("?release=2026.10.18").await), [b]);
    assert_eq!(ids(&p.list("?tag=team%3Asearch").await), [a]);
    assert_eq!(ids(&p.list("?tag=team%3Asupport").await), [b]);
    assert!(ids(&p.list("?tag=team%3A").await).is_empty());
    assert_eq!(ids(&p.list("?prompt=triage-v2").await), [b, a]);
    assert_eq!(
        ids(&p.list("?prompt=triage-v2&environment=production").await),
        [a]
    );
    assert!(ids(&p.list("?prompt=nobody").await).is_empty());
    // A filter given again admits a run matching any of its values; an empty
    // value, a form's untouched field, admits every run.
    assert_eq!(ids(&p.list("?agent=support-bot&agent=zeta").await), [c, b]);
    assert_eq!(ids(&p.list("?status=error&status=ok").await), [c, b, a]);
    assert_eq!(ids(&p.list("?user=user-a&user=user-b").await), [b, a]);
    assert_eq!(
        ids(&p
            .list("?model=gpt-x&model=claude-sonnet-5-5&tool=search_docs")
            .await),
        [a]
    );
    assert_eq!(ids(&p.list("?agent=&user=").await), [c, b, a]);

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
    assert_eq!(ids(&p.list("?sort=session&dir=desc").await), [b, a, c]);
    assert_eq!(ids(&p.list("?sort=status").await)[0], b, "error before ok");
    assert_eq!(ids(&p.list("?sort=started&dir=asc").await), [a, b, c]);
    assert_eq!(
        ids(&p.list("?sort=errors").await),
        [b, a, c],
        "the most first, the newer of two alike"
    );
    assert_eq!(ids(&p.list("?sort=input_tokens").await), [a, b, c]);
    assert_eq!(ids(&p.list("?sort=output_tokens&dir=asc").await), [c, b, a]);
    assert_eq!(ids(&p.list("?sort=tool_calls").await), [a, c, b]);
    assert_eq!(ids(&p.list("?sort=model_calls").await), [c, b, a]);

    // A page of two, and how many the list holds on every page, past the
    // last one too.
    let two = p.list("?limit=2").await;
    assert_eq!(ids(&two), [c, b]);
    assert_eq!(two["more"], true);
    assert_eq!(two["limit"], 2);
    assert_eq!(two["total"], 3);
    let second = p.list("?limit=2&page=2").await;
    assert_eq!(ids(&second), [a]);
    assert_eq!(second["more"], false);
    assert_eq!(second["page"], 2);
    let past = p.list("?limit=2&page=3").await;
    assert!(ids(&past).is_empty());
    assert_eq!(past["total"], 3);
    assert_eq!(p.list("?status=error").await["total"], 1);
    let one_day = p.list("?days=1").await;
    assert_eq!(ids(&one_day).len(), 3);
    assert_eq!(one_day["days"], 1);
}

/// Each facet names its values among the project's runs and how many runs
/// carry each, counted under every filter but its own: a value beside a
/// ticked one still says what ticking it would add, and a ticked value comes
/// first.
#[sqlx::test]
async fn the_facets_count_each_value_under_every_other_filter(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    seed(&ch, &p).await;
    let lane = format!("{LANE}/facets");

    let all = p.get(&lane).await;
    assert_eq!(all["days"], 7);
    assert_eq!(
        values(&all, "agent"),
        owned(&[("nightly-report", 1), ("support-bot", 1), ("zeta", 1)]),
        "another project's run is not counted"
    );
    assert_eq!(values(&all, "status"), owned(&[("ok", 2), ("error", 1)]));
    assert_eq!(
        values(&all, "model"),
        owned(&[("gpt-x", 2), ("claude-sonnet-5-5", 1)])
    );
    assert_eq!(values(&all, "tool"), owned(&[("search_docs", 1)]));
    assert_eq!(values(&all, "user"), owned(&[("user-a", 1), ("user-b", 1)]));
    assert_eq!(
        values(&all, "session"),
        owned(&[("conv-1", 1), ("conv-2", 1)])
    );
    assert_eq!(
        values(&all, "environment"),
        owned(&[("production", 1), ("staging", 1)]),
        "a run from no deployment named is no value"
    );
    assert_eq!(
        values(&all, "release"),
        owned(&[("2026.10.17", 1), ("2026.10.18", 1)])
    );
    assert_eq!(
        values(&all, "tag"),
        owned(&[("team:search", 1), ("team:support", 1)]),
        "a tag as key:value, wherever on the run it was"
    );
    assert_eq!(values(&all, "prompt"), owned(&[("triage-v2", 2)]));
    assert_eq!(all["agent"]["more"], false);

    let by_model = p.get(&format!("{lane}?model=gpt-x")).await;
    assert_eq!(
        values(&by_model, "agent"),
        owned(&[("support-bot", 1), ("zeta", 1)])
    );
    assert_eq!(
        values(&by_model, "model"),
        owned(&[("gpt-x", 2), ("claude-sonnet-5-5", 1)]),
        "the model filter does not narrow its own facet"
    );
    assert_eq!(
        values(&by_model, "status"),
        owned(&[("error", 1), ("ok", 1)])
    );
    assert_eq!(values(&by_model, "environment"), owned(&[("staging", 1)]));

    let by_environment = p.get(&format!("{lane}?environment=production")).await;
    assert_eq!(
        values(&by_environment, "agent"),
        owned(&[("nightly-report", 1)])
    );
    assert_eq!(
        values(&by_environment, "environment"),
        owned(&[("production", 1), ("staging", 1)]),
        "the ticked value first, and the filter not narrowing its own facet"
    );
    assert_eq!(
        values(&by_environment, "prompt"),
        owned(&[("triage-v2", 1)])
    );

    let ticked = p.get(&format!("{lane}?agent=zeta")).await;
    assert_eq!(
        values(&ticked, "agent"),
        owned(&[("zeta", 1), ("nightly-report", 1), ("support-bot", 1)]),
        "the ticked value first"
    );

    let (status, _) = p.send(p.request(STRANGER, &p.ours, &lane)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// The histogram counts each run in the bar its first span started in, the
/// bars starting on the reader's clock and running from the window's opening
/// to now, the empty ones with no durations.
#[sqlx::test]
async fn the_histogram_counts_each_run_in_the_bar_it_started_in(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    let Seeded { a, b, c, .. } = seed(&ch, &p).await;
    // Run E, at :55 five hours back: the last bar, or a shift the wrong way, misplaces it.
    let hour = (Utc::now() - Duration::hours(5))
        .timestamp()
        .div_euclid(3_600)
        * 3_600;
    let e_start = DateTime::from_timestamp(hour + 55 * 60, 0).expect("a time");
    let e = span_at(
        &p.ours,
        Uuid::new_v4(),
        SpanKind::Run,
        "nightly-report",
        e_start,
        1_000,
        9,
    );
    ch.store.insert_spans(&[e]).await.expect("the span lands");

    // Nepal's clock, 5:45 east of UTC: its hours start at UTC's :15, a wrong sign's at :45.
    let histogram = p
        .get(&format!("{LANE}/histogram?days=1&utc_offset=345"))
        .await;
    assert_eq!(histogram["bucket_seconds"], 3_600);
    assert_eq!(histogram["utc_offset"], 345);
    let bars = histogram["bars"].as_array().expect("bars");
    assert!((25..=26).contains(&bars.len()), "{} bars", bars.len());
    let sum = |key: &str| -> u64 { bars.iter().map(|b| b[key].as_u64().expect("a count")).sum() };
    assert_eq!((sum("ok"), sum("error"), sum("cancelled")), (3, 1, 0));
    assert_eq!(sum("tokens"), 1_515);
    let at = |bar: &Value| -> DateTime<Utc> {
        bar["at"]
            .as_str()
            .expect("a time")
            .parse()
            .expect("RFC 3339")
    };
    for bar in bars {
        assert_eq!(
            at(bar).timestamp() % 3_600,
            900,
            "{} starts on the reader's hour",
            at(bar)
        );
    }

    // Each run in the reader's hour it started in, by the list's starts.
    let list = p.list("").await;
    let started = |run: Uuid| -> DateTime<Utc> {
        trace(&list, run)["started_at"]
            .as_str()
            .expect("a time")
            .parse()
            .expect("RFC 3339")
    };
    let reader_hour = |start: DateTime<Utc>| {
        let shift = 345 * 60_000;
        let ms = (start.timestamp_millis() + shift).div_euclid(3_600_000) * 3_600_000 - shift;
        DateTime::from_timestamp_millis(ms).expect("a time")
    };
    let mut expected: BTreeMap<DateTime<Utc>, (u64, u64)> = BTreeMap::new();
    for (start, (ok, error)) in [
        (started(a), (1, 0)),
        (started(b), (0, 1)),
        (started(c), (1, 0)),
        (e_start, (1, 0)),
    ] {
        let bar = expected.entry(reader_hour(start)).or_default();
        *bar = (bar.0 + ok, bar.1 + error);
    }
    let counted: BTreeMap<DateTime<Utc>, (u64, u64)> = bars
        .iter()
        .map(|bar| {
            let count = |key: &str| bar[key].as_u64().expect("a count");
            (at(bar), (count("ok"), count("error")))
        })
        .filter(|(_, counts)| *counts != (0, 0))
        .collect();
    assert_eq!(counted, expected);
    let empty = bars
        .iter()
        .find(|b| b["ok"] == 0 && b["error"] == 0 && b["cancelled"] == 0)
        .expect("an hour no run started in");
    assert_eq!(empty["duration_p50_ms"], Value::Null);

    let failed = p
        .get(&format!("{LANE}/histogram?days=1&status=error"))
        .await;
    let bars = failed["bars"].as_array().expect("bars");
    let errors: u64 = bars
        .iter()
        .map(|b| b["error"].as_u64().expect("a count"))
        .sum();
    let ok: u64 = bars
        .iter()
        .map(|b| b["ok"].as_u64().expect("a count"))
        .sum();
    assert_eq!((errors, ok), (1, 0));
}

/// A download is the list's rows under its filters and sort, as CSV for a
/// spreadsheet or JSON, or the runs chosen by id; a run of another project
/// named by id is not in it.
#[sqlx::test]
async fn a_download_is_the_lists_rows_as_csv_or_json(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    let Seeded { a, b, c, d } = seed(&ch, &p).await;

    let (status, content_type, body) = p.download(MEMBER, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/csv; charset=utf-8");
    let text = String::from_utf8(body).expect("UTF-8");
    let text = text.strip_prefix('\u{feff}').expect("the byte-order mark");
    let lines: Vec<&str> = text.split("\r\n").filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 4, "the header and a row a run");
    assert!(
        lines[0].starts_with("run_id,agent,user_id,session_id,ended,status,error_type,started_at")
    );
    assert!(
        lines[0].ends_with(",models,tools,environments,releases"),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].starts_with(&format!("{c},zeta,,,false,ok,")),
        "{}",
        lines[1]
    );
    assert!(lines[1].ends_with(",gpt-x,,,"), "{}", lines[1]);
    assert!(
        lines[3].ends_with(",claude-sonnet-5-5,search_docs,production,2026.10.17"),
        "{}",
        lines[3]
    );

    let (status, content_type, body) = p.download(MEMBER, "?format=json&sort=tokens").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/json");
    let rows: Value = serde_json::from_slice(&body).expect("JSON");
    let run_ids = |rows: &Value| -> Vec<String> {
        rows.as_array()
            .expect("rows")
            .iter()
            .map(|r| r["run_id"].as_str().expect("a run id").to_owned())
            .collect()
    };
    assert_eq!(run_ids(&rows), [a, b, c].map(|r| r.to_string()));

    let (_, _, body) = p
        .download(MEMBER, &format!("?format=json&run={a}&run={b}&run={d}"))
        .await;
    let chosen: Value = serde_json::from_slice(&body).expect("JSON");
    assert_eq!(
        run_ids(&chosen),
        [b, a].map(|r| r.to_string()),
        "the chosen runs, and none of another project's"
    );

    let page: String = [a, b, c]
        .into_iter()
        .chain((3..RUNS_MAX).map(|_| Uuid::new_v4()))
        .map(|run| format!("&run={run}"))
        .collect();
    let (status, _, body) = p.download(MEMBER, &format!("?format=json{page}")).await;
    assert_eq!(status, StatusCode::OK, "a page's rows, chosen whole");
    let whole: Value = serde_json::from_slice(&body).expect("JSON");
    let mut ids = run_ids(&whole);
    ids.sort();
    let mut ours = [a, b, c].map(|r| r.to_string());
    ours.sort();
    assert_eq!(ids, ours);

    for query in ["?format=xlsx", "?run=not-a-run", "?page=2"] {
        let (status, _, _) = p.download(MEMBER, query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
    }
    let (status, _, _) = p.download(STRANGER, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
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
    assert_eq!(run["sample_rate"], 1.0);
    assert_eq!(run["user_id"], "user-a");
    assert_eq!(run["environment"], "production");
    assert_eq!(run["release"], "2026.10.17");
    let call = &spans[2];
    assert_eq!(call["parent_span_id"], format!("{:016x}", 2));
    assert_eq!(call["model"], "claude-sonnet-5-5");
    assert_eq!(call["input_tokens"], 1_200);
    assert_eq!(
        call["attributes"],
        serde_json::json!({ "gen_ai.response.id": "resp_01", "gen_ai.prompt.name": "triage-v2" })
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

/// A run whose spans hold more units than a page shows is cut where they pass, and says so.
#[sqlx::test]
async fn a_trace_of_wide_spans_is_cut_where_their_units_pass_the_cap(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    let start = Utc::now().trunc_subsecs(6) - Duration::minutes(1);
    let (cut, whole) = (Uuid::new_v4(), Uuid::new_v4());
    let half = |run: Uuid, n: u64| SpanRow {
        units: TRACE_UNITS_MAX / 2,
        ..span_at(
            &p.ours,
            run,
            SpanKind::Step,
            "a wide step",
            start + Duration::milliseconds(i64::try_from(n).expect("small")),
            10,
            n,
        )
    };
    let rows = vec![
        half(cut, 1),
        half(cut, 2),
        half(cut, 3),
        half(whole, 1),
        half(whole, 2),
    ];
    ch.store.insert_spans(&rows).await.expect("the spans land");

    for (run, shown, truncated) in [(cut, 2, true), (whole, 2, false)] {
        let (status, body) = p
            .send(p.request(MEMBER, &p.ours, &format!("{LANE}/{run}")))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["truncated"], truncated, "{run}");
        assert_eq!(
            body["spans"].as_array().expect("spans").len(),
            shown,
            "{run}"
        );
    }
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
        "?page=0",
        "?page=1001",
        "?status=maybe%3Cscript%3E",
        "?tag=untagged%3Cscript%3E",
        "?tag=%3Anameless",
        "?sort=spend",
        "?sort=agent&sort=user",
        "?dir=sideways",
        "?nonsense=1",
        "?format=csv",
        "?utc_offset=60",
        "/facets?sort=agent",
        "/histogram?utc_offset=900",
        "/histogram?page=2",
        "/export?format=xlsx%3Cscript%3E",
        "/export?run=not-a-run",
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
            !body.contains("script")
                && !body.contains("sideways")
                && !body.contains("spend")
                && !body.contains("nameless"),
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
