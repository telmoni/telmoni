//! The conversation lanes against a real ClickHouse: the page sums each
//! conversation's runs into one row, latest first, sorted as asked, a run
//! with no conversation belonging to none and another project's never
//! listed; a conversation's own page is its traces in the order they ran,
//! and a conversation the project's runs in the window did not belong to is
//! a 404.
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

use common::{AuthStub, SECRET, clickhouse_or_skip, run_of, state_with_stores};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::router;

const LANE: &str = "/internal/telemetry/conversations";
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

    /// One conversation's page as the member reads it, by its id as the path
    /// spells it.
    async fn detail(&self, path: &str) -> (StatusCode, Value) {
        self.send(self.request(MEMBER, &self.ours, &format!("{LANE}/{path}")))
            .await
    }
}

/// The conversation ids a page answers, in its order.
fn ids(list: &Value) -> Vec<&str> {
    list["conversations"]
        .as_array()
        .expect("conversations")
        .iter()
        .map(|c| c["conversation_id"].as_str().expect("a conversation id"))
        .collect()
}

/// The page's row for `conversation`.
fn conversation<'a>(list: &'a Value, conversation: &str) -> &'a Value {
    list["conversations"]
        .as_array()
        .expect("conversations")
        .iter()
        .find(|c| c["conversation_id"] == conversation)
        .expect("the conversation is listed")
}

/// The run ids a conversation's page answers, in its order.
fn runs(detail: &Value) -> Vec<Uuid> {
    detail["traces"]
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

/// Five runs of ours — two of one conversation's, one of them failed; one
/// of another's; one whose id holds a slash and a space; one that names no
/// conversation — and one of another project's.
struct Seeded {
    a: Uuid,
    b: Uuid,
    spaced: Uuid,
}

async fn seed(ch: &common::ClickHouse, p: &Projects) -> Seeded {
    let now = Utc::now().trunc_subsecs(6);
    let (a, b, c, spaced) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let mut rows = Vec::new();
    rows.extend(run_of(
        &p.ours,
        a,
        "user-a",
        "conv-1",
        now - Duration::minutes(30),
        (100, 10),
        false,
        0x10,
    ));
    rows.extend(run_of(
        &p.ours,
        b,
        "user-a",
        "conv-1",
        now - Duration::minutes(20),
        (200, 20),
        true,
        0x20,
    ));
    rows.extend(run_of(
        &p.ours,
        c,
        "user-b",
        "conv-2",
        now - Duration::minutes(10),
        (1_000, 100),
        false,
        0x30,
    ));
    rows.extend(run_of(
        &p.ours,
        spaced,
        "user-c",
        "chat/42 b",
        now - Duration::minutes(3),
        (7, 7),
        false,
        0x40,
    ));
    rows.extend(run_of(
        &p.ours,
        Uuid::new_v4(),
        "",
        "",
        now - Duration::minutes(5),
        (50, 5),
        false,
        0x50,
    ));
    rows.extend(run_of(
        &p.theirs,
        Uuid::new_v4(),
        "user-a",
        "conv-9",
        now - Duration::minutes(1),
        (9, 9),
        false,
        0x60,
    ));
    ch.store.insert_spans(&rows).await.expect("the spans land");
    Seeded { a, b, spaced }
}

#[sqlx::test]
async fn the_page_sums_each_conversations_runs_latest_first(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    seed(&ch, &p).await;

    let list = p.list("").await;
    assert_eq!(
        ids(&list),
        ["chat/42 b", "conv-2", "conv-1"],
        "latest first"
    );
    assert_eq!(list["sort"], "last_seen");
    assert_eq!(list["dir"], "desc");
    assert_eq!(list["days"], 7);
    assert_eq!(list["limit"], 100);
    assert_eq!(list["more"], false);

    let one = conversation(&list, "conv-1");
    assert_eq!(one["user_id"], "user-a");
    assert_eq!(one["traces"], 2);
    assert_eq!(one["errors"], 1, "one of its two runs failed");
    assert_eq!(one["steps"], 2);
    assert_eq!(one["input_tokens"], 300);
    assert_eq!(one["output_tokens"], 30);
    assert!(one["first_seen"].as_str().is_some() && one["last_seen"].as_str().is_some());
    assert_eq!(conversation(&list, "conv-2")["user_id"], "user-b");

    let (status, _) = p.send(p.request(STRANGER, &p.ours, LANE)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = p.send(p.request("user_other", &p.theirs, LANE)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids(&body),
        ["conv-9"],
        "theirs sees their one, and none of ours"
    );
}

#[sqlx::test]
async fn the_page_sorts_by_any_column_and_holds_to_its_limit(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    seed(&ch, &p).await;

    assert_eq!(
        ids(&p.list("?sort=conversation").await),
        ["chat/42 b", "conv-1", "conv-2"]
    );
    assert_eq!(
        ids(&p.list("?sort=user").await),
        ["conv-1", "conv-2", "chat/42 b"]
    );
    assert_eq!(
        ids(&p.list("?sort=traces").await),
        ["conv-1", "chat/42 b", "conv-2"],
        "the most runs first, ties latest first"
    );
    assert_eq!(ids(&p.list("?sort=errors").await)[0], "conv-1");
    assert_eq!(
        ids(&p.list("?sort=tokens").await),
        ["conv-2", "conv-1", "chat/42 b"]
    );
    assert_eq!(
        ids(&p.list("?sort=first_seen").await),
        ["conv-1", "conv-2", "chat/42 b"],
        "earliest first"
    );
    assert_eq!(
        ids(&p.list("?sort=last_seen&dir=asc").await),
        ["conv-1", "conv-2", "chat/42 b"]
    );

    let one = p.list("?limit=1").await;
    assert_eq!(ids(&one), ["chat/42 b"]);
    assert_eq!(one["more"], true);

    // The word the page no longer answers to is refused by name, unechoed.
    let (status, body) = p
        .send(p.request(MEMBER, &p.ours, &format!("{LANE}?sort=session")))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .expect("a detail")
            .starts_with("sort")
    );
}

#[sqlx::test]
async fn a_conversation_is_its_traces_in_the_order_they_ran(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    let seeded = seed(&ch, &p).await;

    let (status, detail) = p.detail("conv-1").await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["conversation_id"], "conv-1");
    assert_eq!(runs(&detail), [seeded.a, seeded.b], "earliest first");
    assert_eq!(detail["traces"][0]["agent"], "support-bot");
    assert_eq!(detail["traces"][0]["status"], "ok");
    assert_eq!(detail["traces"][1]["status"], "error");
    assert_eq!(detail["traces"][1]["user_id"], "user-a");
    assert_eq!(detail["days"], 7);
    assert_eq!(detail["more"], false);

    // An id with a slash and a space, as the path encodes it.
    let (status, spaced) = p.detail("chat%2F42%20b").await;
    assert_eq!(status, StatusCode::OK, "{spaced}");
    assert_eq!(spaced["conversation_id"], "chat/42 b");
    assert_eq!(runs(&spaced), [seeded.spaced]);

    let (status, cut) = p.detail("conv-1?limit=1").await;
    assert_eq!(status, StatusCode::OK, "{cut}");
    assert_eq!(runs(&cut), [seeded.a]);
    assert_eq!(cut["more"], true);

    // Another project's conversation, and one nobody had, are not here.
    let (status, body) = p.detail("conv-9").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["type"], "/errors/auth/not-found");
    let (status, _) = p.detail("nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = p.detail(&"x".repeat(1_000)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = p.detail("conv-1?days=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = p
        .send(p.request(STRANGER, &p.ours, &format!("{LANE}/conv-1")))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
