//! Alert precision, through the module's router and its loop: every notice of
//! an incident links the console page it is voted on from; a member's vote, a
//! `POST` alone, is one a person an incident and changed at will; and the
//! week's votes read as the share useful, the project's and each alert's.
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

use common::{AuthStub, NoticeLog, SECRET, clickhouse_or_skip};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::NoLimits;
use telmoni_shared::{
    NotificationKind, OrganizationId, OrganizationRole, ProjectId, Role, SpanKind,
};
use telmoni_telemetry::rules::evaluate;
use telmoni_telemetry::{AppState, router};

const RULES: &str = "/internal/v1/telemetry/rules";
const INCIDENTS: &str = "/internal/v1/telemetry/incidents";
const PRECISION: &str = "/internal/v1/telemetry/precision";

struct Project {
    app: axum::Router,
    state: Arc<AppState>,
    notices: Arc<NoticeLog>,
    organization: OrganizationId,
    project: ProjectId,
}

impl Project {
    /// A project of two members, its notices linking the console where
    /// `console` asks.
    async fn new(pool: &PgPool, ch: &common::ClickHouse, console: bool) -> Self {
        let auth = AuthStub::new();
        let (organization, project) = (OrganizationId::new(), ProjectId::new());
        for user in ["user_member", "user_other"] {
            auth.seat(
                user,
                &organization,
                &project,
                OrganizationRole::Member,
                Some(Role::Member),
            );
        }
        let notices = NoticeLog::new();
        let state = if console {
            common::state_with_console(pool, auth, notices.clone())
        } else {
            common::state_with_stores_noticed(pool, auth, Arc::new(NoLimits), notices.clone())
        };
        ch.store
            .insert_spans(&[common::received(
                &project,
                SpanKind::ModelCall,
                "chat claude-sonnet-5-5",
                1_200,
                Utc::now() - TimeDelta::seconds(2),
            )])
            .await
            .expect("the span is kept");
        Self {
            app: router(state.clone()),
            state,
            notices,
            organization,
            project,
        }
    }

    async fn send_as(
        &self,
        user: &str,
        method: &str,
        uri: &str,
        body: Option<&Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", format!("Bearer tok_{user}"))
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", self.project.as_str())
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .expect("a request");
        let response = self.app.clone().oneshot(request).await.expect("an answer");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, body)
    }

    async fn send(&self, method: &str, uri: &str, body: Option<&Value>) -> (StatusCode, Value) {
        self.send_as("user_member", method, uri, body).await
    }

    /// An alert on the project's model calls that breaches at once, evaluated:
    /// its incident's id.
    async fn incident(&self, pool: &PgPool, name: &str) -> String {
        let body = json!({
            "name": name, "kind": "model_call", "measure": "count", "aggregation": "count",
            "window": "hour", "operator": "gt", "alert_threshold": 0.0,
        });
        let (status, made) = self.send("POST", RULES, Some(&body)).await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
        sqlx::query("UPDATE telemetry.rules SET evaluated_at = NULL")
            .execute(pool)
            .await
            .expect("due again");
        evaluate::pass(&self.state).await.expect("a pass");
        let (_, listed) = self.send("GET", INCIDENTS, None).await;
        listed
            .get("incidents")
            .and_then(Value::as_array)
            .and_then(|all| {
                all.iter()
                    .find(|one| one.get("rule_name") == Some(&json!(name)))
            })
            .and_then(|one| one.get("id"))
            .and_then(Value::as_str)
            .expect("its incident")
            .to_owned()
    }

    async fn vote(&self, user: &str, incident: &str, verdict: &str) -> (StatusCode, Value) {
        self.send_as(
            user,
            "POST",
            &format!("{INCIDENTS}/{incident}/vote"),
            Some(&json!({ "verdict": verdict })),
        )
        .await
    }
}

/// What a vote or a read answers of the tally: useful, noise and the asker's.
fn tally(answer: &Value) -> (Value, Value, Value) {
    let field = |name: &str| answer.get(name).cloned().unwrap_or(Value::Null);
    (field("useful"), field("noise"), field("yours"))
}

/// ⚠ Every notice of an incident, opened or resolved, links the console page
/// it is voted on from; without the console's origin a notice links none.
#[sqlx::test]
async fn every_notice_of_an_incident_links_its_vote_page(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, true).await;
    let incident = project.incident(&pool, "Calls").await;
    let (status, _) = project
        .send("POST", &format!("{INCIDENTS}/{incident}/resolve"), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    let page = format!(
        "https://console.example/{}/{}/alerts/incidents/{incident}/vote",
        project.organization, project.project
    );
    let told = project.notices.notices();
    let kinds: Vec<NotificationKind> = told.iter().map(|notice| notice.kind).collect();
    assert_eq!(
        kinds,
        [
            NotificationKind::IncidentOpened,
            NotificationKind::IncidentResolved
        ]
    );
    for notice in &told {
        assert!(
            notice
                .body
                .ends_with(&format!("\nUseful, or noise? {page}")),
            "{}",
            notice.body
        );
        assert_eq!(notice.metadata.get("vote_url"), Some(&json!(page)));
    }

    let bare = Project::new(&pool, &ch, false).await;
    bare.incident(&pool, "Calls").await;
    let told = bare.notices.notices();
    assert_eq!(told.len(), 1);
    assert!(
        !told[0].body.contains("Useful, or noise?"),
        "{}",
        told[0].body
    );
    assert_eq!(told[0].metadata.get("vote_url"), Some(&Value::Null));
}

/// ⚠ A member's vote is one a person an incident: given, changed, and read
/// back with everyone's.
#[sqlx::test]
async fn a_member_votes_once_an_incident_and_may_change_their_word(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, true).await;
    let incident = project.incident(&pool, "Calls").await;

    let (status, voted) = project.vote("user_member", &incident, "useful").await;
    assert_eq!(status, StatusCode::OK, "{voted}");
    assert_eq!(tally(&voted), (json!(1), json!(0), json!("useful")));
    let (_, changed) = project.vote("user_member", &incident, "noise").await;
    assert_eq!(tally(&changed), (json!(0), json!(1), json!("noise")));
    let (_, other) = project.vote("user_other", &incident, "useful").await;
    assert_eq!(tally(&other), (json!(1), json!(1), json!("useful")));

    let (status, read) = project
        .send("GET", &format!("{INCIDENTS}/{incident}"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(tally(&read), (json!(1), json!(1), json!("noise")));
    assert_eq!(read.get("rule_name"), Some(&json!("Calls")));
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM telemetry.incident_votes")
        .fetch_one(&pool)
        .await
        .expect("the votes");
    assert_eq!(rows, 2, "one a person");
}

/// ⚠ A vote is a `POST` on an incident of the project, with a word it knows;
/// anything else casts nothing.
#[sqlx::test]
async fn a_vote_is_a_post_on_an_incident_of_the_project(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, true).await;
    let incident = project.incident(&pool, "Calls").await;
    let elsewhere = Project::new(&pool, &ch, true).await;
    let theirs = elsewhere.incident(&pool, "Calls").await;

    let (status, _) = project
        .send("GET", &format!("{INCIDENTS}/{incident}/vote"), None)
        .await;
    assert_eq!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "a link followed casts nothing"
    );
    for (id, verdict, expected) in [
        (theirs.as_str(), "useful", StatusCode::NOT_FOUND),
        (&Uuid::new_v4().to_string(), "useful", StatusCode::NOT_FOUND),
        ("not-an-id", "useful", StatusCode::NOT_FOUND),
    ] {
        let (status, refused) = project.vote("user_member", id, verdict).await;
        assert_eq!(status, expected, "{id}: {refused}");
    }
    let (status, refused) = project.vote("user_member", &incident, "maybe").await;
    assert!(status.is_client_error(), "{status} {refused}");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM telemetry.incident_votes")
        .fetch_one(&pool)
        .await
        .expect("the votes");
    assert_eq!(rows, 0, "nothing cast");
}

/// ⚠ The week's votes read as the share useful, the project's and each
/// alert's, an alert without a vote at none; an incident opened before the
/// week is left out.
#[sqlx::test]
async fn the_weeks_votes_read_as_the_share_useful_alert_by_alert(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, true).await;
    let calls = project.incident(&pool, "Calls").await;
    let more = project.incident(&pool, "More calls").await;
    project.incident(&pool, "Quiet").await;
    project.vote("user_member", &calls, "useful").await;
    project.vote("user_other", &calls, "useful").await;
    project.vote("user_member", &more, "noise").await;

    let (status, read) = project.send("GET", PRECISION, None).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(
        (read.get("days"), read.get("useful"), read.get("noise")),
        (Some(&json!(7)), Some(&json!(2)), Some(&json!(1)))
    );
    let share = read.get("ratio").and_then(Value::as_f64).expect("a share");
    assert!((share - 2.0 / 3.0).abs() < 1e-9, "{share}");
    let rules: Vec<(Value, Value, Value)> = read
        .get("rules")
        .and_then(Value::as_array)
        .expect("each alert")
        .iter()
        .map(|rule| {
            let field = |name: &str| rule.get(name).cloned().unwrap_or(Value::Null);
            (field("rule_name"), field("noise"), field("ratio"))
        })
        .collect();
    assert_eq!(
        rules,
        [
            (json!("Calls"), json!(0), json!(1.0)),
            (json!("More calls"), json!(1), json!(0.0)),
            (json!("Quiet"), json!(0), Value::Null),
        ]
    );

    sqlx::query(
        "UPDATE telemetry.incidents SET opened_at = now() - interval '8 days' WHERE id = $1",
    )
    .bind(Uuid::parse_str(&more).expect("an id"))
    .execute(&pool)
    .await
    .expect("opened last week");
    let (_, read) = project.send("GET", PRECISION, None).await;
    assert_eq!(
        (read.get("useful"), read.get("noise")),
        (Some(&json!(2)), Some(&json!(0)))
    );
}
