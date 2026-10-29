//! The session lanes against a real ClickHouse: the page sums each session's
//! runs into one row, latest first, sorted, paged and filtered as asked, a
//! run with no session id belonging to none and another project's never
//! listed, its facets count sessions and its download holds them all; a
//! session's own page is its traces in the order they ran, and a session the
//! project's runs in the window did not belong to is a 404.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, SubsecRound, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{
    AuthStub, FILTER_QUERIES, SECRET, clickhouse_or_skip, run_of, state_with_stores, varied_runs,
};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::router;

const LANE: &str = "/internal/v1/telemetry/sessions";
const MEMBER: &str = "user_member";
const OWNER: &str = "user_owner";
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
    auth.seat(OWNER, &organization, &ours, OrganizationRole::Owner, None);
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

    /// A request with a body, as the console sends one on ours.
    fn on(&self, method: &str, user: &str, path: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", format!("Bearer tok_{user}"))
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", self.ours.as_str())
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
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

    /// One session's page as the member reads it, by its id as the path spells
    /// it.
    async fn detail(&self, path: &str) -> (StatusCode, Value) {
        self.send(self.request(MEMBER, &self.ours, &format!("{LANE}/{path}")))
            .await
    }

    /// A layout saved on ours by its owner, pinning `pinned`, by its id.
    async fn save_layout(&self, name: &str, pinned: Value) -> String {
        let body = json!({ "name": name, "kind": "layout", "query": { "pinned": pinned } });
        let (status, saved) = self
            .send(self.on("POST", OWNER, "/internal/v1/telemetry/views", body))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{saved}");
        saved
            .get("id")
            .and_then(Value::as_str)
            .expect("an id")
            .to_owned()
    }

    /// The layout ours opens its sessions in, set by its owner.
    async fn set_layout(&self, view_id: &str) {
        let path = "/internal/v1/telemetry/session-layout";
        let (status, body) = self
            .send(self.on("PUT", OWNER, path, json!({ "view_id": view_id })))
            .await;
        assert!(status.is_success(), "{status}: {body}");
    }

    /// The page's facets as the member reads them, with `query`.
    async fn facets(&self, query: &str) -> Value {
        let path = format!("/internal/v1/telemetry/session-facets{query}");
        let (status, body) = self.send(self.request(MEMBER, &self.ours, &path)).await;
        assert_eq!(status, StatusCode::OK, "{query}: {body}");
        body
    }

    /// The page's download as the member makes it, with `query`: its status,
    /// its content type and its body.
    async fn export(&self, query: &str) -> (StatusCode, String, String) {
        let path = format!("/internal/v1/telemetry/session-export{query}");
        let response = self
            .app
            .clone()
            .oneshot(self.request(MEMBER, &self.ours, &path))
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

/// The session ids a page answers, in its order.
fn ids(list: &Value) -> Vec<&str> {
    list["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|s| s["session_id"].as_str().expect("a session id"))
        .collect()
}

/// The page's row for `session`.
fn session<'a>(list: &'a Value, session: &str) -> &'a Value {
    list["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|s| s["session_id"] == session)
        .expect("the session is listed")
}

/// The run ids a session's page answers, in its order.
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

/// Five runs of ours — two of one session's, one of them failed; one of
/// another's; one whose id holds a slash and a space; one that names no
/// session — and one of another project's.
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
async fn the_page_sums_each_sessions_runs_latest_first(pool: PgPool) {
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
    assert_eq!(list["page"], 1);
    assert_eq!(list["total"], 3, "the run that names no session is none's");
    assert_eq!(list["more"], false);

    let one = session(&list, "conv-1");
    assert_eq!(one["user_id"], "user-a");
    assert_eq!(one["traces"], 2);
    assert_eq!(one["errors"], 1, "one of its two runs failed");
    assert_eq!(one["steps"], 2);
    assert_eq!(one["input_tokens"], 300);
    assert_eq!(one["output_tokens"], 30);
    assert!(one["first_seen"].as_str().is_some() && one["last_seen"].as_str().is_some());
    assert_eq!(session(&list, "conv-2")["user_id"], "user-b");

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
async fn the_page_sorts_by_any_column_and_pages(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    seed(&ch, &p).await;

    assert_eq!(
        ids(&p.list("?sort=session").await),
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
        ids(&p.list("?sort=input_tokens&dir=asc").await),
        ["chat/42 b", "conv-1", "conv-2"]
    );
    assert_eq!(
        ids(&p.list("?sort=output_tokens").await),
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
    assert_eq!(one["total"], 3);
    assert_eq!(one["more"], true);
    let second = p.list("?limit=2&page=2").await;
    assert_eq!(ids(&second), ["conv-1"]);
    assert_eq!(second["page"], 2);
    assert_eq!(second["more"], false);
    let past = p.list("?limit=2&page=3").await;
    assert!(ids(&past).is_empty());
    assert_eq!(
        past["total"], 3,
        "a page past the last still counts the rest"
    );

    // The word the page no longer answers to is refused by name, unechoed.
    let (status, body) = p
        .send(p.request(MEMBER, &p.ours, &format!("{LANE}?sort=conversation")))
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
async fn a_session_is_its_traces_in_the_order_they_ran(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    let seeded = seed(&ch, &p).await;

    let (status, detail) = p.detail("conv-1").await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["session_id"], "conv-1");
    assert_eq!(runs(&detail), [seeded.a, seeded.b], "earliest first");
    assert_eq!(detail["traces"][0]["agent"], "support-bot");
    assert_eq!(detail["traces"][0]["status"], "ok");
    assert_eq!(detail["traces"][0]["error_type"], "");
    assert_eq!(detail["traces"][1]["status"], "error");
    assert_eq!(
        detail["traces"][1]["error_type"], "RuntimeError",
        "the run's own error class, never its message"
    );
    assert_eq!(detail["traces"][1]["user_id"], "user-a");
    assert_eq!(detail["pinned"], json!([]));
    assert_eq!(detail["traces"][0]["pinned"], json!([]));
    assert_eq!(detail["days"], 7);
    assert_eq!(detail["more"], false);

    // The spans a layout pinned by name stand under each trace: the step
    // every run of the seed took, and nothing for a name no span has.
    let (status, pinned) = p
        .detail("conv-1?pinned=answer&pinned=nothing&pinned=answer")
        .await;
    assert_eq!(status, StatusCode::OK, "{pinned}");
    assert_eq!(pinned["pinned"], json!(["answer", "nothing"]));
    assert_eq!(runs(&pinned), [seeded.a, seeded.b]);
    // Each run's own step, under its own run span, by the seed's ids.
    let traces = pinned["traces"].as_array().expect("traces");
    for (trace, run_span) in traces.iter().zip([0x10_u64, 0x20]) {
        let spans = trace["pinned"].as_array().expect("pinned spans");
        assert_eq!(spans.len(), 1, "{trace}");
        assert_eq!(spans[0]["name"], "answer");
        assert_eq!(spans[0]["kind"], "step");
        assert_eq!(spans[0]["status"], "ok");
        assert!(spans[0]["started_at"].as_str().is_some());
        assert_eq!(spans[0]["span_id"], format!("{:016x}", run_span + 1));
        assert_eq!(spans[0]["parent_span_id"], format!("{run_span:016x}"));
    }
    let nine = (0..=8)
        .map(|i| format!("pinned=step-{i}"))
        .collect::<Vec<_>>()
        .join("&");
    let (status, body) = p.detail(&format!("conv-1?{nine}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = p.detail("conv-1?sort=started").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a parameter the lane does not take"
    );

    // An id with a slash and a space, as the path encodes it.
    let (status, spaced) = p.detail("chat%2F42%20b").await;
    assert_eq!(status, StatusCode::OK, "{spaced}");
    assert_eq!(spaced["session_id"], "chat/42 b");
    assert_eq!(runs(&spaced), [seeded.spaced]);

    let (status, cut) = p.detail("conv-1?limit=1").await;
    assert_eq!(status, StatusCode::OK, "{cut}");
    assert_eq!(runs(&cut), [seeded.a]);
    assert_eq!(cut["more"], true);

    // Another project's session, and one nobody had, are not here.
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

/// The trace list's filters admit a session that holds a run they admit, and
/// the row still sums every run of it in the window.
#[sqlx::test]
async fn a_filter_admits_a_session_by_one_run_and_sums_it_whole(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    seed(&ch, &p).await;

    let failed = p.list("?status=error").await;
    assert_eq!(ids(&failed), ["conv-1"]);
    assert_eq!(failed["total"], 1);
    let one = session(&failed, "conv-1");
    assert_eq!(one["traces"], 2, "its run that ended well is summed too");
    assert_eq!(one["input_tokens"], 300);
    assert_eq!(ids(&p.list("?user=user-b").await), ["conv-2"]);
    assert_eq!(
        ids(&p.list("?session=conv-1&session=chat%2F42%20b").await),
        ["chat/42 b", "conv-1"]
    );
    let none = p.list("?agent=billing-bot").await;
    assert!(ids(&none).is_empty());
    assert_eq!(none["total"], 0);

    // Another agent's run in conv-2 lets the session in, summed with the
    // support bot's run beside it.
    let mut billed = run_of(
        &p.ours,
        Uuid::new_v4(),
        "user-b",
        "conv-2",
        Utc::now().trunc_subsecs(6) - Duration::minutes(2),
        (5, 5),
        false,
        0x70,
    );
    for span in &mut billed {
        span.agent = "billing-bot".into();
    }
    ch.store
        .insert_spans(&billed)
        .await
        .expect("the spans land");
    let billing = p.list("?agent=billing-bot").await;
    assert_eq!(ids(&billing), ["conv-2"]);
    assert_eq!(session(&billing, "conv-2")["traces"], 2);
    assert_eq!(session(&billing, "conv-2")["input_tokens"], 1_005);
}

/// Every filter lists the sessions of the runs the trace list admits: the
/// one reads a run's matches as counts, the other off the arrays it shows.
#[sqlx::test]
async fn the_filters_list_the_sessions_of_the_runs_the_trace_list_admits(pool: PgPool) {
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
            .filter_map(|t| t["session_id"].as_str().filter(|s| !s.is_empty()))
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
        3,
        "the run with no session is none's"
    );
    assert_eq!(ids(&p.list("?tool=search_docs").await), ["conv-2"]);
}

/// Each facet counts the sessions that hold a run carrying a value, under
/// every filter but its own.
#[sqlx::test]
async fn the_facets_count_sessions_under_every_other_filter(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    seed(&ch, &p).await;

    let all = p.facets("").await;
    assert_eq!(
        values(&all, "status"),
        [("ok".to_owned(), 3), ("error".to_owned(), 1)],
        "three sessions hold a run that ended well; one, a run that failed"
    );
    assert_eq!(values(&all, "agent"), [("support-bot".to_owned(), 3)]);
    assert_eq!(all["days"], 7);
    let failed = p.facets("?status=error").await;
    assert_eq!(
        values(&failed, "status"),
        [("error".to_owned(), 1), ("ok".to_owned(), 3)],
        "a facet's own filter does not narrow it, the ticked value first"
    );
    assert_eq!(values(&failed, "user"), [("user-a".to_owned(), 1)]);

    let (status, _) = p
        .send(p.request(
            MEMBER,
            &p.ours,
            "/internal/v1/telemetry/session-facets?sort=traces",
        ))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// The download is every session the filters admit, sorted as asked, as CSV
/// or JSON, and takes no chosen runs.
#[sqlx::test]
async fn the_download_holds_every_session_the_filters_admit(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    seed(&ch, &p).await;

    let (status, kind, csv) = p.export("?sort=session").await;
    assert_eq!(status, StatusCode::OK, "{csv}");
    assert_eq!(kind, "text/csv; charset=utf-8");
    let lines: Vec<&str> = csv.trim_start_matches('\u{feff}').lines().collect();
    assert_eq!(
        lines[0],
        "session_id,user_id,traces,errors,steps,input_tokens,output_tokens,duration_ms,first_seen,\
         last_seen"
    );
    assert_eq!(lines.len(), 4, "a header and three sessions: {csv}");
    assert!(lines[1].starts_with("chat/42 b,user-c,1,0,1,7,7,"), "{csv}");

    let (status, kind, json) = p.export("?status=error&format=json").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(kind, "application/json");
    let rows: Value = serde_json::from_str(&json).expect("JSON");
    assert_eq!(rows.as_array().map(Vec::len), Some(1));
    assert_eq!(rows[0]["session_id"], "conv-1");
    assert_eq!(rows[0]["traces"], 2);
    assert!(
        rows[0]["duration_ms"].as_i64().is_some_and(|ms| ms >= 0),
        "{json}"
    );

    for refused in [
        "?run=6f2b8e3c-5d1a-4b7e-9c0f-1a2b3c4d5e6f",
        "?page=2",
        "?format=xlsx",
    ] {
        let (status, _, body) = p.export(refused).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
    }
}

/// A session read by a layout rather than by names: the project's own pins
/// what its saved layout pins, a link's layout its own where it is the
/// project's, one the project lacks the project's, and nothing where no
/// layout is set; a layout is never read beside names.
#[sqlx::test]
async fn a_session_is_read_with_the_layout_a_link_names_else_the_projects_own(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let p = projects(&pool);
    let seeded = seed(&ch, &p).await;

    let (status, bare) = p.detail("conv-1?view=project").await;
    assert_eq!(status, StatusCode::OK, "{bare}");
    assert_eq!(bare["pinned"], json!([]), "no layout set: nothing pinned");
    assert_eq!(runs(&bare), [seeded.a, seeded.b]);

    let review = p.save_layout("Review", json!(["answer"])).await;
    let plain = p.save_layout("Plain", json!([])).await;
    p.set_layout(&review).await;
    let (status, own) = p.detail("conv-1?view=project").await;
    assert_eq!(status, StatusCode::OK, "{own}");
    assert_eq!(own["pinned"], json!(["answer"]));
    assert_eq!(own["traces"][0]["pinned"][0]["name"], "answer");
    let (status, linked) = p.detail(&format!("conv-1?view={plain}")).await;
    assert_eq!(status, StatusCode::OK, "{linked}");
    assert_eq!(
        linked["pinned"],
        json!([]),
        "the link's layout over the project's"
    );
    let (status, unknown) = p.detail(&format!("conv-1?view={}", Uuid::new_v4())).await;
    assert_eq!(status, StatusCode::OK, "{unknown}");
    assert_eq!(
        unknown["pinned"],
        json!(["answer"]),
        "a layout the project lacks: its own"
    );
    for refused in ["conv-1?view=nope", "conv-1?view=project&pinned=answer"] {
        let (status, body) = p.detail(refused).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
    }
}
