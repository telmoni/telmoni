//! The user lanes against a real ClickHouse: every user id the project's
//! runs were for in the window, each summed into one row, sorted, paged and
//! filtered as asked, a run with no user nobody's and another project's never
//! listed, the facets counting users and the download holding them all; a
//! query a parameter does not take is a 400 that echoes none of it.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Duration, SubsecRound, Utc};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{
    AuthStub, FILTER_QUERIES, SECRET, clickhouse_or_skip, run_of, state_with_stores, varied_runs,
};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::router;

const LANE: &str = "/internal/v1/telemetry/users";
const MEMBER: &str = "user_member";
const STRANGER: &str = "user_stranger";

/// Two projects: ours, with a member seated, and another's.
struct Projects {
    app: Router,
    organization: OrganizationId,
    ours: ProjectId,
    theirs: ProjectId,
}

fn projects(pool: &PgPool) -> Projects {
    let (organization, ours, theirs) = (OrganizationId::new(), ProjectId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.seat(
        MEMBER,
        &organization,
        &ours,
        OrganizationRole::Member,
        Some(Role::Member),
    );
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

    /// The page as the member reads it, with `query` (`"?sort=…"` or empty).
    async fn list(&self, query: &str) -> Value {
        let (status, body) = self
            .send(self.request(MEMBER, &self.ours, &format!("{LANE}{query}")))
            .await;
        assert_eq!(status, StatusCode::OK, "{query}: {body}");
        body
    }

    /// The page's facets as the member reads them, with `query`.
    async fn facets(&self, query: &str) -> Value {
        let (status, body) = self
            .send(self.request(MEMBER, &self.ours, &format!("{LANE}/facets{query}")))
            .await;
        assert_eq!(status, StatusCode::OK, "{query}: {body}");
        body
    }

    /// The page's download as the member makes it, with `query`: its status,
    /// its content type and its body.
    async fn export(&self, query: &str) -> (StatusCode, String, String) {
        let response = self
            .app
            .clone()
            .oneshot(self.request(MEMBER, &self.ours, &format!("{LANE}/export{query}")))
            .await
            .expect("the router answers");
        let status = response.status();
        let kind = response
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
        (
            status,
            kind,
            String::from_utf8(bytes.to_vec()).expect("text"),
        )
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
                v["count"].as_u64().expect("a count"),
            )
        })
        .collect()
}

/// The user ids a page answers, in its order.
fn ids(list: &Value) -> Vec<&str> {
    list["users"]
        .as_array()
        .expect("users")
        .iter()
        .map(|u| u["user_id"].as_str().expect("a user id"))
        .collect()
}

/// The page's row for the user `id`.
fn user<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["users"]
        .as_array()
        .expect("users")
        .iter()
        .find(|u| u["user_id"] == id)
        .expect("the user is listed")
}

/// Four runs of ours — two of one user's, in one session, one of them
/// failed; one of another's; one that names nobody — and one of another
/// project's, by the first user.
struct Seeded {
    a_started: DateTime<Utc>,
    b_ended: DateTime<Utc>,
    c_ended: DateTime<Utc>,
}

async fn seed(ch: &common::ClickHouse, p: &Projects) -> Seeded {
    let now = Utc::now().trunc_subsecs(6);
    let a_started = now - Duration::minutes(30);
    let b_started = now - Duration::minutes(20);
    let c_started = now - Duration::minutes(10);
    let mut rows = Vec::new();
    rows.extend(run_of(
        &p.ours,
        Uuid::new_v4(),
        "user-a",
        "conv-1",
        a_started,
        (100, 10),
        false,
        0x10,
    ));
    rows.extend(run_of(
        &p.ours,
        Uuid::new_v4(),
        "user-a",
        "conv-1",
        b_started,
        (200, 20),
        true,
        0x20,
    ));
    rows.extend(run_of(
        &p.ours,
        Uuid::new_v4(),
        "user-b",
        "conv-2",
        c_started,
        (1_000, 100),
        false,
        0x30,
    ));
    rows.extend(run_of(
        &p.ours,
        Uuid::new_v4(),
        "",
        "",
        now - Duration::minutes(5),
        (50, 5),
        false,
        0x40,
    ));
    rows.extend(run_of(
        &p.theirs,
        Uuid::new_v4(),
        "user-a",
        "conv-9",
        now - Duration::minutes(1),
        (9, 9),
        false,
        0x50,
    ));
    ch.store.insert_spans(&rows).await.expect("the spans land");
    Seeded {
        a_started,
        b_ended: b_started + Duration::seconds(2),
        c_ended: c_started + Duration::seconds(2),
    }
}

#[sqlx::test]
async fn the_page_sums_each_users_runs_the_most_spent_on_first(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    let seeded = seed(&ch, &p).await;

    let list = p.list("").await;
    assert_eq!(ids(&list), ["user-b", "user-a"], "the most tokens first");
    assert_eq!(list["sort"], "tokens");
    assert_eq!(list["dir"], "desc");
    assert_eq!(list["days"], 7);
    assert_eq!(list["limit"], 100);
    assert_eq!(list["page"], 1);
    assert_eq!(list["total"], 2, "the run that names nobody is nobody's");
    assert_eq!(list["more"], false);

    let a = user(&list, "user-a");
    assert_eq!(a["traces"], 2);
    assert_eq!(a["errors"], 1, "one of the two runs failed");
    assert_eq!(a["steps"], 2);
    assert_eq!(a["input_tokens"], 300);
    assert_eq!(a["output_tokens"], 30);
    assert_eq!(
        a["first_seen"]
            .as_str()
            .expect("a time")
            .parse::<DateTime<Utc>>()
            .expect("RFC 3339"),
        seeded.a_started
    );
    assert_eq!(
        a["last_seen"]
            .as_str()
            .expect("a time")
            .parse::<DateTime<Utc>>()
            .expect("RFC 3339"),
        seeded.b_ended
    );
    let b = user(&list, "user-b");
    assert_eq!(b["traces"], 1);
    assert_eq!(b["errors"], 0);
    assert_eq!(b["input_tokens"], 1_000);
    assert_eq!(
        b["last_seen"]
            .as_str()
            .expect("a time")
            .parse::<DateTime<Utc>>()
            .expect("RFC 3339"),
        seeded.c_ended
    );

    let (status, _) = p.send(p.request(STRANGER, &p.ours, LANE)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = p.send(p.request("user_other", &p.theirs, LANE)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids(&body),
        ["user-a"],
        "theirs sees their one run, and none of ours"
    );
    assert_eq!(user(&body, "user-a")["traces"], 1);
}

#[sqlx::test]
async fn the_page_sorts_by_any_column_and_pages(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    seed(&ch, &p).await;

    assert_eq!(ids(&p.list("?sort=user").await), ["user-a", "user-b"]);
    assert_eq!(
        ids(&p.list("?sort=user&dir=desc").await),
        ["user-b", "user-a"]
    );
    assert_eq!(ids(&p.list("?sort=traces").await), ["user-a", "user-b"]);
    assert_eq!(ids(&p.list("?sort=errors").await), ["user-a", "user-b"]);
    assert_eq!(ids(&p.list("?sort=steps").await), ["user-a", "user-b"]);
    assert_eq!(
        ids(&p.list("?sort=tokens&dir=asc").await),
        ["user-a", "user-b"]
    );
    assert_eq!(
        ids(&p.list("?sort=input_tokens").await),
        ["user-b", "user-a"]
    );
    assert_eq!(
        ids(&p.list("?sort=output_tokens&dir=asc").await),
        ["user-a", "user-b"]
    );
    assert_eq!(
        ids(&p.list("?sort=first_seen").await),
        ["user-a", "user-b"],
        "earliest first"
    );
    assert_eq!(
        ids(&p.list("?sort=last_seen").await),
        ["user-b", "user-a"],
        "latest first"
    );

    let one = p.list("?limit=1").await;
    assert_eq!(ids(&one), ["user-b"]);
    assert_eq!(one["more"], true);
    assert_eq!(one["limit"], 1);
    assert_eq!(one["total"], 2);
    let second = p.list("?limit=1&page=2").await;
    assert_eq!(ids(&second), ["user-a"]);
    assert_eq!(second["page"], 2);
    assert_eq!(second["more"], false);
    assert_eq!(p.list("?days=1").await["days"], 1);
}

/// Every filter lists the users of the runs the trace list admits: the one
/// reads a run's matches as counts, the other off the arrays it shows.
#[sqlx::test]
async fn the_filters_list_the_users_of_the_runs_the_trace_list_admits(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    ch.store
        .insert_spans(&varied_runs(&p.ours))
        .await
        .expect("the spans land");
    for query in FILTER_QUERIES {
        let path = format!("/internal/v1/telemetry/traces{query}");
        let (status, traces) = p.send(p.request(MEMBER, &p.ours, &path)).await;
        assert_eq!(status, StatusCode::OK, "{query}: {traces}");
        let mut expected: Vec<&str> = traces["traces"]
            .as_array()
            .expect("traces")
            .iter()
            .filter_map(|t| t["user_id"].as_str().filter(|u| !u.is_empty()))
            .collect();
        expected.sort_unstable();
        expected.dedup();
        let list = p.list(query).await;
        let mut listed = ids(&list);
        listed.sort_unstable();
        assert_eq!(listed, expected, "{query}");
        assert_eq!(list["total"], listed.len(), "{query}");
    }
    assert_eq!(
        ids(&p.list("").await).len(),
        4,
        "a user's run with no session is theirs all the same"
    );
    assert_eq!(ids(&p.list("?model=gpt-5").await), ["user-c"]);
}

/// The trace list's filters admit a user one of whose runs they admit, and
/// the row still sums every run of theirs in the window; the facets count
/// users, and the download holds every user the filters admit.
#[sqlx::test]
async fn the_filters_facets_and_download_read_users(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    seed(&ch, &p).await;

    let failed = p.list("?status=error").await;
    assert_eq!(ids(&failed), ["user-a"]);
    assert_eq!(failed["total"], 1);
    assert_eq!(
        user(&failed, "user-a")["traces"],
        2,
        "their run that ended well is summed too"
    );
    assert_eq!(ids(&p.list("?session=conv-2").await), ["user-b"]);
    assert_eq!(ids(&p.list("?user=user-a").await), ["user-a"]);
    assert!(ids(&p.list("?model=gpt-0").await).is_empty());

    let all = p.facets("").await;
    assert_eq!(
        values(&all, "status"),
        [("ok".to_owned(), 2), ("error".to_owned(), 1)]
    );
    assert_eq!(
        values(&all, "model"),
        [("claude-sonnet-5-5".to_owned(), 2)],
        "two users, whatever their runs number"
    );
    assert_eq!(
        values(&p.facets("?status=error").await, "session"),
        [("conv-1".to_owned(), 1)]
    );

    let (status, kind, csv) = p.export("?sort=user").await;
    assert_eq!(status, StatusCode::OK, "{csv}");
    assert_eq!(kind, "text/csv; charset=utf-8");
    let lines: Vec<&str> = csv.trim_start_matches('\u{feff}').lines().collect();
    assert_eq!(
        lines[0],
        "user_id,traces,errors,steps,input_tokens,output_tokens,duration_ms,first_seen,last_seen"
    );
    assert_eq!(lines.len(), 3, "a header and two users: {csv}");
    assert!(lines[1].starts_with("user-a,2,1,2,300,30,"), "{csv}");
    let (status, kind, json) = p.export("?status=error&format=json").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(kind, "application/json");
    let rows: Value = serde_json::from_str(&json).expect("JSON");
    assert_eq!(rows.as_array().map(Vec::len), Some(1));
    assert_eq!(rows[0]["user_id"], "user-a");
    for refused in ["?run=6f2b8e3c-5d1a-4b7e-9c0f-1a2b3c4d5e6f", "?limit=5"] {
        let (status, _, body) = p.export(refused).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
    }
}

/// A value a parameter does not take is a 400 that names the parameter and
/// never echoes the value.
#[sqlx::test]
async fn a_bad_query_is_refused_without_an_echo(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    for (query, parameter) in [
        ("?sort=spend%3Cscript%3E", "sort"),
        ("?dir=sideways", "dir"),
        ("?days=0", "days"),
        ("?days=361", "days"),
        ("?limit=0", "limit"),
        ("?limit=501", "limit"),
        ("?page=1001", "page"),
        ("?status=maybe%3Cscript%3E", "status"),
    ] {
        let (status, body) = p
            .send(p.request(MEMBER, &p.ours, &format!("{LANE}{query}")))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
        let detail = body["detail"].as_str().expect("a detail");
        assert!(detail.starts_with(parameter), "{query}: {detail}");
        assert!(
            !detail.contains("script") && !detail.contains("sideways"),
            "{detail}"
        );
    }
    let (status, _) = p
        .send(p.request(MEMBER, &p.ours, &format!("{LANE}?nonsense=1")))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let no_secret = Request::builder()
        .method("GET")
        .uri(LANE)
        .header("authorization", format!("Bearer tok_{MEMBER}"))
        .body(Body::empty())
        .expect("a request");
    let (status, _) = p.send(no_secret).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
