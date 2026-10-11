//! The users lane against a real ClickHouse: every user id the project's
//! runs were for in the window, each summed into one row, sorted as asked,
//! a run with no user nobody's and another project's never listed; a
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

use common::{AuthStub, SECRET, clickhouse_or_skip, run_of, state_with_stores};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::router;

const LANE: &str = "/internal/telemetry/users";
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

/// Four runs of ours — two of one user's, in one conversation, one of them
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
async fn the_page_sorts_by_any_column_and_holds_to_its_limit(pool: PgPool) {
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
    assert_eq!(p.list("?days=1").await["days"], 1);
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
