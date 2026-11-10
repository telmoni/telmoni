//! The canary suite: one corpus of fake but realistic secrets and personal
//! data, `sdk/fixtures/canary.json`, sent through every way out of the server,
//! each value found only where it may be. With content `off`, nowhere: not
//! ingest's answer, ClickHouse, a Postgres table in plain text, a log line, a
//! notice, the trace's page, its download or `/v1`. With content `on`, sealed in
//! Postgres and shown only to a reader of the trace's page, its download and
//! `/v1`, each read filed. Already scrubbed by an SDK, only its markers and
//! its counts. Planted in metadata, where the field inventory says.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use common::{AuthStub, NoticeLog, clickhouse_or_skip};
use telmoni_shared::middleware::api_key::{ApiKeyGate, refuse_as_problem, require_api_key};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::NoLimits;
use telmoni_shared::test_util::{apply_audit_migrations, captured_logs};
use telmoni_shared::{ApiKeyScope, ContentMode, OrganizationId, OrganizationRole, ProjectId};
use telmoni_telemetry::rules::evaluate;
use telmoni_telemetry::store::Projects;
use telmoni_telemetry::{AppState, contract, otlp_router, router, v1_reads, v1_router};

const KEY: &str = "tm_canary_ingest_key";
const READ_KEY: &str = "tm_canary_read_key";
const TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4742";
const SPAN: &str = "00f067aa0ba90242";
const OWNER: &str = "owner-1";

#[sqlx::test]
async fn measured_speed_agrees_on_public_and_internal_metadata_reads(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::Off).await;
    let at = chrono::Utc::now() - chrono::Duration::minutes(1);
    ch.store
        .insert_spans(&[telmoni_telemetry::store::SpanRow {
            trace_id: TRACE.into(),
            span_id: SPAN.into(),
            started_at: at,
            ended_at: at + chrono::Duration::seconds(3),
            output_tokens: 101,
            time_to_first_token: Some(1.0),
            ..common::span(
                &door.project,
                telmoni_shared::SpanKind::ModelCall,
                "chat",
                3000,
            )
        }])
        .await
        .expect("measured call lands");
    for path in [format!("/traces/{TRACE}"), format!("/v1/traces/{TRACE}")] {
        let (status, body) = door.get(&path).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        let body: Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(body["spans"][0]["output_speed"], 50.0);
    }
    assert_eq!(
        door.viewed(&pool).await,
        0,
        "metadata read opens no content"
    );
}

/// The corpus, as every SDK's suite and every plugin's reads it.
fn corpus() -> Value {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/fixtures/canary.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("the corpus is readable"))
        .expect("the corpus is JSON")
}

/// Each planted value as it is searched for: line by line, so a key's lines
/// are found wherever its newlines were escaped.
fn needles(corpus: &Value) -> Vec<(String, String)> {
    corpus["planted"]
        .as_object()
        .expect("the planted values")
        .iter()
        .flat_map(|(kind, value)| {
            value
                .as_str()
                .expect("a planted value is text")
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| (kind.clone(), line.to_owned()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The case whose text holds every planted value.
fn every_planted(corpus: &Value) -> &Value {
    corpus["cases"]
        .as_array()
        .expect("the cases")
        .iter()
        .find(|case| case["name"] == "every planted value in one prose text")
        .expect("the case of every planted value")
}

struct Door {
    app: Router,
    state: Arc<AppState>,
    notices: Arc<NoticeLog>,
    organization: OrganizationId,
    project: ProjectId,
}

/// The module's lanes over both stores, `/v1`'s reads behind their key gate as
/// auth's router puts them, on a project whose settings row says `mode`.
async fn door(pool: &PgPool, mode: ContentMode) -> Door {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    auth.key(READ_KEY, &organization, &project, &[ApiKeyScope::Read]);
    auth.seat(
        OWNER,
        &organization,
        &project,
        OrganizationRole::Owner,
        None,
    );
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id, content_mode) \
         VALUES ($1, $2, $3)",
    )
    .bind(&project)
    .bind(&organization)
    .bind(mode)
    .execute(pool)
    .await
    .expect("seed the project's mode");
    let notices = NoticeLog::new();
    let state = common::state_with_stores_noticed(pool, auth, Arc::new(NoLimits), notices.clone());
    let gate = ApiKeyGate {
        auth: state.auth.clone(),
        scope: ApiKeyScope::Read,
        refuse: refuse_as_problem,
    };
    let reads = v1_reads(state.clone())
        .route_layer(axum::middleware::from_fn_with_state(gate, require_api_key));
    Door {
        app: router(state.clone())
            .nest("/otlp", otlp_router(state.clone()))
            .nest("/v1", v1_router(state.clone()).merge(reads)),
        state,
        notices,
        organization,
        project,
    }
}

/// A failed trace whose content fields, its status's message and an attribute
/// of the application's own carry `text`, beside `named` attributes.
fn export(text: &str, named: &[(&str, &str)]) -> Vec<u8> {
    let messages =
        json!([{ "role": "user", "parts": [{ "type": "text", "content": text }] }]).to_string();
    let mut attributes = vec![
        json!({ "key": contract::AGENT, "value": { "stringValue": "canary-agent" } }),
        json!({ "key": contract::OPERATION, "value": { "stringValue": "invoke_agent" } }),
        json!({ "key": "gen_ai.input.messages", "value": { "stringValue": messages } }),
        json!({ "key": "gen_ai.system_instructions", "value": { "stringValue": text } }),
        json!({ "key": "app.note", "value": { "stringValue": text } }),
    ];
    attributes.extend(
        named
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": { "stringValue": value } })),
    );
    let now = chrono::Utc::now();
    let nanos =
        |at: chrono::DateTime<chrono::Utc>| at.timestamp_nanos_opt().expect("in range").to_string();
    json!({ "resourceSpans": [{ "resource": { "attributes": [{ "key": contract::DISTRO_NAME, "value": { "stringValue": "telmoni" } }] }, "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": [{
        "traceId": TRACE, "spanId": SPAN, "name": "canary trace", "kind": 1,
        "startTimeUnixNano": nanos(now - chrono::Duration::minutes(2)),
        "endTimeUnixNano": nanos(now - chrono::Duration::minutes(1)),
        "status": { "code": 2, "message": text },
        "attributes": attributes
    }]}]}]})
    .to_string()
    .into_bytes()
}

impl Door {
    /// An export, its answer's text.
    async fn send(&self, body: Vec<u8>) -> String {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/otlp/v1/traces")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::from(body))
                    .expect("a request"),
            )
            .await
            .expect("an answer");
        assert_eq!(response.status(), StatusCode::OK);
        text_of(response).await
    }

    /// One of the console's lanes as the project's owner, or `/v1` with the read key.
    async fn get(&self, path: &str) -> (StatusCode, String) {
        let request = if path.starts_with("/v1") {
            Request::builder()
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {READ_KEY}"))
        } else {
            Request::builder()
                .uri(format!("/internal/v1/telemetry{path}"))
                .header(SERVICE_SECRET_HEADER, common::SECRET)
                .header(header::AUTHORIZATION, format!("Bearer tok_{OWNER}"))
                .header("x-organization-id", self.organization.as_str())
                .header("x-project-id", self.project.as_str())
        };
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::empty()).expect("a request"))
            .await
            .expect("an answer");
        let status = response.status();
        (status, text_of(response).await)
    }

    async fn post_json(&self, path: &str, body: &Value) -> StatusCode {
        let request = Request::builder()
            .method("POST")
            .uri(format!("/internal/v1/telemetry{path}"))
            .header(SERVICE_SECRET_HEADER, common::SECRET)
            .header(header::AUTHORIZATION, format!("Bearer tok_{OWNER}"))
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", self.project.as_str())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("a request");
        self.app
            .clone()
            .oneshot(request)
            .await
            .expect("an answer")
            .status()
    }

    async fn viewed(&self, pool: &PgPool) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM audit.events WHERE organization_id = $1 AND action = 'viewed'",
        )
        .bind(&self.organization)
        .fetch_one(pool)
        .await
        .expect("read the chain")
    }

    /// The project's spans and hourly totals, every column as text, its rows
    /// that hold `needle`.
    async fn in_clickhouse(&self, ch: &common::ClickHouse, needle: &str) -> u64 {
        let only = Projects::one(self.project.clone());
        let mut carrying = 0;
        for every_column in [
            "SELECT countIf(position(formatRow('TSVRaw', *), {needle:String}) > 0) \
               FROM telemetry.spans WHERE project_id IN {projects:Array(String)}",
            "SELECT countIf(position(formatRow('TSVRaw', *), {needle:String}) > 0) \
               FROM telemetry.spans_hourly WHERE project_id IN {projects:Array(String)}",
        ] {
            let with: u64 = ch
                .store
                .read(&only, every_column)
                .param("needle", needle)
                .fetch_one()
                .await
                .expect("search every column of the project's rows");
            carrying += with;
        }
        carrying
    }
}

async fn text_of(response: axum::response::Response) -> String {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Every table of the stores the server keeps in Postgres, each whole as text.
async fn postgres_text(pool: &PgPool) -> Vec<(String, String)> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_schema || '.' || table_name FROM information_schema.tables \
          WHERE table_schema IN ('telemetry', 'audit') AND table_type = 'BASE TABLE'",
    )
    .fetch_all(pool)
    .await
    .expect("list the tables");
    let mut texts = Vec::new();
    for table in tables {
        let text: String = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT coalesce(string_agg(t::text, ' '), '') FROM {table} t"
        )))
        .fetch_one(pool)
        .await
        .expect("read the table whole");
        texts.push((table, text));
    }
    texts
}

/// Every notice handed to notifications, as text.
fn notices_text(notices: &NoticeLog) -> String {
    notices
        .notices()
        .iter()
        .map(|notice| format!("{} {} {}", notice.title, notice.body, notice.metadata))
        .collect::<Vec<_>>()
        .join("\n")
}

/// ⚠ With content off, no planted value is kept or shown anywhere; with it
/// on, each is kept sealed and shown only to a reader of the trace's page, its
/// download and `/v1`, each read filed, and nowhere else: not ingest's
/// answer, ClickHouse, Postgres in plain text, a log line or a notice.
#[sqlx::test]
async fn the_canary_corpus_is_kept_and_shown_only_where_each_mode_lets_it(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let (_logging, logs) = captured_logs();
    let corpus = corpus();
    let needles = needles(&corpus);
    let text = every_planted(&corpus)["text"]
        .as_str()
        .expect("the case's text");
    let off = door(&pool, ContentMode::Off).await;
    let on = door(&pool, ContentMode::On).await;

    let mut answers = Vec::new();
    for door in [&off, &on] {
        answers.push(door.send(export(text, &[])).await);
        common::eventually("the trace is read", || async {
            door.get("/traces").await.1.contains(TRACE)
        })
        .await;
        let failing = json!({ "name": "Any failure", "kind": "agent", "measure": "failures",
            "aggregation": "count", "window": "hour", "operator": "gt", "alert_threshold": 0 });
        assert_eq!(
            door.post_json("/rules", &failing).await,
            StatusCode::CREATED
        );
        evaluate::pass(&door.state).await.expect("a pass");
        assert!(
            !door.notices.notices().is_empty(),
            "the failed trace opened an incident and told it"
        );
    }

    // What a reader is shown: nothing of an off project's, all of an on one's, each read filed.
    let reads = [
        format!("/traces/{TRACE}?content=true"),
        "/traces/export?format=json".to_owned(),
        format!("/v1/traces/{TRACE}?content=true"),
    ];
    for read in &reads {
        let (status, shown) = off.get(read).await;
        assert_eq!(status, StatusCode::OK, "{read}: {shown}");
        for (kind, needle) in &needles {
            assert!(
                !shown.contains(needle.as_str()),
                "{kind} shown by {read} of an off project"
            );
        }
        let (status, shown) = on.get(read).await;
        assert_eq!(status, StatusCode::OK, "{read}: {shown}");
        for (kind, needle) in &needles {
            assert!(
                shown.contains(needle.as_str()),
                "{kind} not shown by {read} of an on project"
            );
        }
    }
    assert_eq!(
        off.viewed(&pool).await,
        0,
        "nothing of an off project read, nothing filed"
    );
    assert_eq!(
        on.viewed(&pool).await,
        3,
        "each read of the on project's content filed"
    );

    // And nowhere else.
    let tables = postgres_text(&pool).await;
    assert!(
        tables
            .iter()
            .any(|(table, _)| table == "telemetry.plain_content")
    );
    let logged = logs.text();
    for door in [&off, &on] {
        let told = notices_text(&door.notices);
        for (kind, needle) in &needles {
            assert_eq!(
                door.in_clickhouse(&ch, needle).await,
                0,
                "{kind} in ClickHouse"
            );
            assert!(!told.contains(needle.as_str()), "{kind} in a notice");
        }
    }
    for (kind, needle) in &needles {
        assert!(!logged.contains(needle.as_str()), "{kind} in a log line");
        assert!(
            !answers
                .iter()
                .any(|answer| answer.contains(needle.as_str())),
            "{kind} in ingest's answer"
        );
        for (table, text) in &tables {
            assert!(
                !text.contains(needle.as_str()),
                "{kind} in {table} in plain text"
            );
        }
    }
}

/// ⚠ Content an SDK scrubbed arrives as its markers and its counts alone: the
/// trace's page shows the markers and no planted value, and the span keeps the
/// counts as metadata, which the project's posture counts.
#[sqlx::test]
async fn content_an_sdk_scrubbed_arrives_as_its_markers_and_its_counts(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let corpus = corpus();
    let needles = needles(&corpus);
    let case = every_planted(&corpus);
    let scrubbed = case["expect"]["plugin_default"]["text"]
        .as_str()
        .expect("the scrubbed text");
    let counts = case["expect"]["plugin_default"]["scrubbed"].to_string();
    let on = door(&pool, ContentMode::On).await;
    on.send(export(scrubbed, &[(contract::SCRUBBED, &counts)]))
        .await;
    common::eventually("the trace is read", || async {
        on.get("/traces").await.1.contains(TRACE)
    })
    .await;

    let (status, page) = on.get(&format!("/traces/{TRACE}?content=true")).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    for (kind, needle) in &needles {
        assert!(
            !page.contains(needle.as_str()),
            "{kind} on the trace's page"
        );
        assert_eq!(
            on.in_clickhouse(&ch, needle).await,
            0,
            "{kind} in ClickHouse"
        );
    }
    for marker in [
        "[secret: aws_key]",
        "[secret: jwt]",
        "[pii: email]",
        "[pii: card]",
    ] {
        assert!(page.contains(marker), "{marker} on the trace's page");
    }
    let shown: Value = serde_json::from_str(&page).expect("the page's answer");
    let span = shown["spans"]
        .as_array()
        .and_then(|spans| spans.iter().find(|span| span["span_id"] == SPAN))
        .expect("the trace's span");
    assert_eq!(
        serde_json::from_str::<Value>(
            span["attributes"][contract::SCRUBBED]
                .as_str()
                .expect("the counts kept")
        )
        .expect("JSON"),
        case["expect"]["plugin_default"]["scrubbed"],
        "the counts, kept as metadata"
    );
    let (status, posture) = on.get("/privacy").await;
    assert_eq!(status, StatusCode::OK, "{posture}");
    assert!(
        posture.contains(&format!("\"attribute\":\"{}\"", contract::SCRUBBED)),
        "the posture counts the scrubbed span: {posture}"
    );
}

/// Door replacements survive sealing and every content read without leaking the planted values.
#[sqlx::test]
async fn foreign_content_is_scrubbed_before_it_is_kept(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let (_logging, logs) = captured_logs();
    let corpus = corpus();
    let text = every_planted(&corpus)["text"].as_str().expect("text");
    let on = door(&pool, ContentMode::On).await;
    sqlx::query(
        "UPDATE telemetry.project_settings SET door_scrub_pii = true WHERE project_id = $1",
    )
    .bind(&on.project)
    .execute(&pool)
    .await
    .expect("enable personal data scrubbing");
    let mut payload: Value = serde_json::from_slice(&export(text, &[])).expect("export");
    payload["resourceSpans"][0]["resource"] = json!({});
    payload["resourceSpans"][0]["scopeSpans"][0]["scope"]["name"] = json!("stock-exporter");
    payload["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["attributes"]
        .as_array_mut()
        .expect("attributes")
        .retain(|attribute| attribute["key"] != contract::AGENT);
    let answer = on.send(payload.to_string().into_bytes()).await;
    let staged: String =
        sqlx::query_scalar("SELECT row::text FROM telemetry.held_spans WHERE project_id = $1")
            .bind(&on.project)
            .fetch_one(&pool)
            .await
            .expect("held span");
    assert!(staged.contains(contract::SCRUBBED));
    for (kind, needle) in needles(&corpus) {
        assert!(!staged.contains(&needle), "{kind} in staged metadata");
    }
    sqlx::query("UPDATE telemetry.project_settings SET door_scrub_secrets = false, door_scrub_pii = false WHERE project_id = $1")
        .bind(&on.project).execute(&pool).await.expect("policy changed after receipt");
    sqlx::query("UPDATE telemetry.held_spans SET received_at = now() - interval '2 minutes' WHERE project_id = $1")
        .bind(&on.project).execute(&pool).await.expect("quiet");
    telmoni_telemetry::foreign::land_quiet(&on.state)
        .await
        .expect("land staged content");
    common::eventually("the trace is read", || async {
        on.get("/traces").await.1.contains(TRACE)
    })
    .await;
    let failing = json!({ "name": "Any failure", "kind": "agent", "measure": "failures",
        "aggregation": "count", "window": "hour", "operator": "gt", "alert_threshold": 0 });
    assert_eq!(on.post_json("/rules", &failing).await, StatusCode::CREATED);
    evaluate::pass(&on.state)
        .await
        .expect("evaluate the failed trace");
    assert!(
        !on.notices.notices().is_empty(),
        "the failed trace generated a notice"
    );
    for path in [
        format!("/traces/{TRACE}?content=true"),
        "/traces/export?format=json".into(),
        format!("/v1/traces/{TRACE}?content=true"),
    ] {
        let (status, shown) = on.get(&path).await;
        assert_eq!(status, StatusCode::OK, "{shown}");
        assert!(shown.contains("[secret: aws_key]"));
        assert!(shown.contains("[pii: email]"));
        for (kind, needle) in needles(&corpus) {
            assert!(!shown.contains(&needle), "{kind} in {path}");
            assert!(!answer.contains(&needle), "{kind} in answer");
            assert_eq!(
                on.in_clickhouse(&ch, &needle).await,
                0,
                "{kind} in ClickHouse"
            );
            assert!(
                !notices_text(&on.notices).contains(&needle),
                "{kind} in notices"
            );
        }
    }
    let logged = logs.text();
    for (kind, needle) in needles(&corpus) {
        assert!(!logged.contains(&needle), "{kind} in logs");
    }
}

/// ⚠ A value planted in metadata is kept where the field inventory says and
/// nowhere else: a step's name and a tag's value as they were sent, a session's
/// and a user's id only as the project's hash of them, and none in a log line.
#[sqlx::test]
async fn a_value_planted_in_metadata_is_kept_where_the_inventory_says(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let (_logging, logs) = captured_logs();
    let corpus = corpus();
    let planted = |kind: &str| {
        corpus["planted"][kind]
            .as_str()
            .expect("planted")
            .to_owned()
    };
    let (name, tag, session, user) = (
        planted("email"),
        planted("phone"),
        planted("ssn"),
        planted("card"),
    );
    let off = door(&pool, ContentMode::Off).await;
    let mut body: Value = serde_json::from_slice(&export(
        "nothing personal",
        &[
            (contract::SESSION_ID, &session),
            (contract::USER_ID, &user),
            ("telmoni.tag.contact", &tag),
        ],
    ))
    .expect("the export");
    // A trace is named for its agent, so the name is planted on its step, kept as sent.
    let step = {
        let mut step = body["resourceSpans"][0]["scopeSpans"][0]["spans"][0].clone();
        step["spanId"] = json!("00f067aa0ba90243");
        step["parentSpanId"] = json!(SPAN);
        step["name"] = json!(name);
        step["attributes"] = json!([
            { "key": contract::AGENT, "value": { "stringValue": "canary-agent" } }
        ]);
        step
    };
    body["resourceSpans"][0]["scopeSpans"][0]["spans"]
        .as_array_mut()
        .expect("the export's spans")
        .push(step);
    off.send(body.to_string().into_bytes()).await;
    common::eventually("the trace is read", || async {
        off.get("/traces").await.1.contains(TRACE)
    })
    .await;

    assert!(
        off.in_clickhouse(&ch, &name).await > 0,
        "a step's name, kept as sent"
    );
    assert!(
        off.in_clickhouse(&ch, &tag).await > 0,
        "a tag's value, kept as sent"
    );
    assert_eq!(
        off.in_clickhouse(&ch, &session).await,
        0,
        "a session's id, kept as its hash"
    );
    assert_eq!(
        off.in_clickhouse(&ch, &user).await,
        0,
        "a user's id, kept as its hash"
    );
    let logged = logs.text();
    for value in [&name, &tag, &session, &user] {
        assert!(
            !logged.contains(value.as_str()),
            "a planted value in a log line"
        );
    }
    for (table, text) in postgres_text(&pool).await {
        for value in [&session, &user] {
            assert!(!text.contains(value.as_str()), "an id sent in {table}");
        }
    }
}
