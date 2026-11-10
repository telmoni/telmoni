//! An agent's changes, found by the module's loop on a clock set by hand and
//! read through its lanes: a new release marked once, and again in another
//! environment; a model seen again inside its 30 days marked not at all, and
//! past them again; the day's cap, its 24th saying so and what it held back
//! not taken for new the next day; a prompt's version under its name; an
//! agent's first trace; another exporter's trace landed late, marked by its own
//! first trace; a pass after an outage going on from the last; one replica
//! writing, and a project a move holds left; the purges; and the lanes,
//! the newest first and held to their project.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, TimeDelta, Utc};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, ClickHouse, NoticeLog, SECRET, clickhouse_or_skip};
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::middleware::api_key::{ApiKeyGate, refuse_as_problem, require_api_key};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::{ApiKeyScope, OrganizationId, OrganizationRole, ProjectId, Role, SpanKind};
use telmoni_telemetry::db::{self, TelemetryLane};
use telmoni_telemetry::store::SpanRow;
use telmoni_telemetry::{AppState, changes, router, v1_reads};

const AGENT: &str = "support";
const MEMBER: &str = "user_member";
const READ_KEY: &str = "tm_read_only_key";
const INGEST_KEY: &str = "tm_ingest_key";

fn at(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .expect("a time")
        .with_timezone(&Utc)
}

/// The module over both stores, a project with its settings row, a member
/// seated on it, and a key of each scope.
struct Watched {
    ch: ClickHouse,
    state: Arc<AppState>,
    organization: OrganizationId,
    project: ProjectId,
}

async fn watched(pool: &PgPool, test: &str) -> Option<Watched> {
    let ch = clickhouse_or_skip(test).await?;
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.seat(
        MEMBER,
        &organization,
        &project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    auth.key(READ_KEY, &organization, &project, &[ApiKeyScope::Read])
        .key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id) VALUES ($1, $2)",
    )
    .bind(&project)
    .bind(&organization)
    .execute(pool)
    .await
    .expect("a settings row");
    let state = common::state_with_console(pool, auth, NoticeLog::new());
    Some(Watched {
        ch,
        state,
        organization,
        project,
    })
}

/// A span of `kind` of the agent's trace `trace`, sent from production on
/// `release`, received at `received` and started a second before.
fn sent(
    project: &ProjectId,
    trace: Uuid,
    kind: SpanKind,
    release: &str,
    received: DateTime<Utc>,
) -> SpanRow {
    SpanRow {
        trace_id: trace.simple().to_string(),
        agent: AGENT.into(),
        environment: "production".into(),
        release: release.into(),
        ..common::received(project, kind, AGENT, 1_000, received)
    }
}

/// A model call of trace `trace` answered by `model`, with `attributes`.
fn call(
    project: &ProjectId,
    trace: Uuid,
    model: &str,
    attributes: &[(&str, &str)],
    received: DateTime<Utc>,
) -> SpanRow {
    SpanRow {
        model: model.into(),
        attributes: attributes
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
        ..sent(project, trace, SpanKind::ModelCall, "", received)
    }
}

async fn insert(w: &Watched, spans: &[SpanRow]) {
    w.ch.store.insert_spans(spans).await.expect("kept");
}

/// A tick at `now`, the last pass ended a minute before it.
async fn tick(w: &Watched, pool: &PgPool, now: DateTime<Utc>) {
    last_pass(pool, now - TimeDelta::minutes(1)).await;
    changes::tick_at(&w.state, now).await.expect("a tick");
}

async fn last_pass(pool: &PgPool, at: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO telemetry.reads (name, read_to) VALUES ($1, $2)
         ON CONFLICT (name) DO UPDATE SET read_to = EXCLUDED.read_to",
    )
    .bind(changes::HEARTBEAT)
    .bind(at)
    .execute(pool)
    .await
    .expect("the last pass");
}

/// A row as the table holds it: environment, kind, prompt, value, trace, mark.
type Stored = (String, String, Option<String>, String, String, String);

/// The project's rows, in the order they changed.
async fn stored(pool: &PgPool, project: &ProjectId) -> Vec<Stored> {
    sqlx::query_as(
        "SELECT environment, kind, prompt, value, trace_id, mark FROM telemetry.changes
          WHERE project_id = $1 ORDER BY changed_at, kind, value",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("its changes")
}

fn row(environment: &str, kind: &str, prompt: Option<&str>, value: &str, trace: Uuid) -> Stored {
    (
        environment.into(),
        kind.into(),
        prompt.map(Into::into),
        value.into(),
        trace.simple().to_string(),
        "shown".into(),
    )
}

/// ⚠ The item's first proof: a trace on a new release makes one row, by its
/// trace and at its start; the next pass, reading the same spans, makes none;
/// staging taking it later makes staging's own.
#[sqlx::test]
async fn a_new_release_is_marked_once_and_again_in_another_environment(pool: PgPool) {
    let Some(w) = watched(&pool, module_path!()).await else {
        return;
    };
    let first = Uuid::new_v4();
    insert(
        &w,
        &[sent(
            &w.project,
            first,
            SpanKind::Agent,
            "1.4.2",
            at("2026-11-02T10:00:00Z"),
        )],
    )
    .await;
    tick(&w, &pool, at("2026-11-02T10:01:00Z")).await;
    assert_eq!(
        stored(&pool, &w.project).await,
        [row("production", "release", None, "1.4.2", first)]
    );
    let changed_at: DateTime<Utc> =
        sqlx::query_scalar("SELECT changed_at FROM telemetry.changes WHERE project_id = $1")
            .bind(&w.project)
            .fetch_one(&pool)
            .await
            .expect("its time");
    assert_eq!(changed_at, at("2026-11-02T09:59:59Z"), "the trace's start");

    tick(&w, &pool, at("2026-11-02T10:02:00Z")).await;
    assert_eq!(
        stored(&pool, &w.project).await.len(),
        1,
        "read again, kept once"
    );

    let (again, staged) = (Uuid::new_v4(), Uuid::new_v4());
    insert(
        &w,
        &[
            sent(
                &w.project,
                again,
                SpanKind::Agent,
                "1.4.2",
                at("2026-11-02T10:05:00Z"),
            ),
            SpanRow {
                environment: "staging".into(),
                ..sent(
                    &w.project,
                    staged,
                    SpanKind::Agent,
                    "1.4.2",
                    at("2026-11-02T10:06:00Z"),
                )
            },
        ],
    )
    .await;
    tick(&w, &pool, at("2026-11-02T10:07:00Z")).await;
    assert_eq!(
        stored(&pool, &w.project).await,
        [
            row("production", "release", None, "1.4.2", first),
            row("staging", "release", None, "1.4.2", staged),
        ]
    );
}

/// ⚠ The item's second proof: a model seen again 20 days on is no change,
/// the time it was seen moved; seen again 31 days after that, it is one.
#[sqlx::test]
async fn a_model_seen_again_inside_its_thirty_days_is_marked_not_at_all(pool: PgPool) {
    let Some(w) = watched(&pool, module_path!()).await else {
        return;
    };
    let first = Uuid::new_v4();
    insert(
        &w,
        &[call(
            &w.project,
            first,
            "gpt-5",
            &[],
            at("2026-10-01T10:00:00Z"),
        )],
    )
    .await;
    tick(&w, &pool, at("2026-10-01T10:01:00Z")).await;
    assert_eq!(
        stored(&pool, &w.project).await,
        [row("production", "model", None, "gpt-5", first)]
    );

    insert(
        &w,
        &[call(
            &w.project,
            Uuid::new_v4(),
            "gpt-5",
            &[],
            at("2026-10-21T10:00:00Z"),
        )],
    )
    .await;
    tick(&w, &pool, at("2026-10-21T10:01:00Z")).await;
    assert_eq!(stored(&pool, &w.project).await.len(), 1);
    let seen_at: DateTime<Utc> =
        sqlx::query_scalar("SELECT seen_at FROM telemetry.changes WHERE project_id = $1")
            .bind(&w.project)
            .fetch_one(&pool)
            .await
            .expect("its last arrival");
    assert_eq!(seen_at, at("2026-10-21T10:00:00Z"));

    let returned = Uuid::new_v4();
    insert(
        &w,
        &[call(
            &w.project,
            returned,
            "gpt-5",
            &[],
            at("2026-11-21T10:00:00Z"),
        )],
    )
    .await;
    tick(&w, &pool, at("2026-11-21T10:01:00Z")).await;
    assert_eq!(
        stored(&pool, &w.project).await,
        [
            row("production", "model", None, "gpt-5", first),
            row("production", "model", None, "gpt-5", returned),
        ]
    );
}

/// ⚠ The item's third proof: 25 new releases in a UTC day make 24 shown, the
/// 24th saying more came, the 25th remembered; a 26th that day is not
/// marked, a new model that day is, and the next day the release held back
/// is no change, while a new one is.
#[sqlx::test]
async fn an_agent_past_24_changes_of_a_kind_in_a_day_is_marked_no_further(pool: PgPool) {
    let Some(w) = watched(&pool, module_path!()).await else {
        return;
    };
    let spans: Vec<SpanRow> = (1..=25)
        .map(|n| {
            sent(
                &w.project,
                Uuid::new_v4(),
                SpanKind::Agent,
                &format!("v{n:02}"),
                at(&format!("2026-11-03T10:00:{n:02}Z")),
            )
        })
        .collect();
    insert(&w, &spans).await;
    tick(&w, &pool, at("2026-11-03T10:01:00Z")).await;
    let marks = |rows: &[Stored]| -> Vec<(String, String)> {
        rows.iter()
            .map(|(_, _, _, value, _, mark)| (value.clone(), mark.clone()))
            .collect()
    };
    let rows = stored(&pool, &w.project).await;
    let mut expected: Vec<(String, String)> = (1..=23)
        .map(|n| (format!("v{n:02}"), "shown".to_owned()))
        .collect();
    expected.push(("v24".into(), "capped".into()));
    expected.push(("v25".into(), "hidden".into()));
    assert_eq!(marks(&rows), expected);

    insert(
        &w,
        &[
            sent(
                &w.project,
                Uuid::new_v4(),
                SpanKind::Agent,
                "v26",
                at("2026-11-03T10:02:00Z"),
            ),
            call(
                &w.project,
                Uuid::new_v4(),
                "gpt-5",
                &[],
                at("2026-11-03T10:02:30Z"),
            ),
        ],
    )
    .await;
    tick(&w, &pool, at("2026-11-03T10:03:00Z")).await;
    let rows = stored(&pool, &w.project).await;
    let mark_of = |rows: &[Stored], value: &str| {
        rows.iter()
            .find(|row| row.3 == value)
            .map(|row| row.5.clone())
    };
    assert_eq!(mark_of(&rows, "v26").as_deref(), Some("hidden"));
    assert_eq!(
        mark_of(&rows, "gpt-5").as_deref(),
        Some("shown"),
        "another kind"
    );
    assert_eq!(
        rows.iter().filter(|row| row.5 == "capped").count(),
        1,
        "said once"
    );

    let next_day = Uuid::new_v4();
    insert(
        &w,
        &[
            sent(
                &w.project,
                Uuid::new_v4(),
                SpanKind::Agent,
                "v25",
                at("2026-11-04T10:00:00Z"),
            ),
            sent(
                &w.project,
                next_day,
                SpanKind::Agent,
                "v27",
                at("2026-11-04T10:00:30Z"),
            ),
        ],
    )
    .await;
    tick(&w, &pool, at("2026-11-04T10:01:00Z")).await;
    let rows = stored(&pool, &w.project).await;
    assert_eq!(
        rows.iter().filter(|row| row.3 == "v25").count(),
        1,
        "held back, not new"
    );
    assert_eq!(mark_of(&rows, "v27").as_deref(), Some("shown"));
}

/// A prompt's version is marked under its name, each version and each name
/// apart; a name with no version, a version with no name and empty values
/// mark nothing.
#[sqlx::test]
async fn a_prompts_version_is_marked_under_its_name(pool: PgPool) {
    let Some(w) = watched(&pool, module_path!()).await else {
        return;
    };
    let (three, four, summarize) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let prompt = |name, version| {
        [
            ("gen_ai.prompt.name", name),
            ("gen_ai.prompt.version", version),
        ]
    };
    insert(
        &w,
        &[
            call(
                &w.project,
                three,
                "",
                &prompt("triage", "3"),
                at("2026-11-04T09:00:01Z"),
            ),
            call(
                &w.project,
                four,
                "",
                &prompt("triage", "4"),
                at("2026-11-04T09:00:02Z"),
            ),
            call(
                &w.project,
                summarize,
                "",
                &prompt("summarize", "3"),
                at("2026-11-04T09:00:03Z"),
            ),
            call(
                &w.project,
                Uuid::new_v4(),
                "",
                &[("gen_ai.prompt.name", "unversioned")],
                at("2026-11-04T09:00:04Z"),
            ),
            call(
                &w.project,
                Uuid::new_v4(),
                "",
                &[("gen_ai.prompt.version", "9")],
                at("2026-11-04T09:00:05Z"),
            ),
            call(
                &w.project,
                Uuid::new_v4(),
                "",
                &prompt("", ""),
                at("2026-11-04T09:00:06Z"),
            ),
        ],
    )
    .await;
    tick(&w, &pool, at("2026-11-04T09:01:00Z")).await;
    assert_eq!(
        stored(&pool, &w.project).await,
        [
            row("production", "prompt", Some("triage"), "3", three),
            row("production", "prompt", Some("triage"), "4", four),
            row("production", "prompt", Some("summarize"), "3", summarize),
        ]
    );
}

/// An agent's first trace marks what it ran on: its release, its model and
/// its prompt's version.
#[sqlx::test]
async fn an_agents_first_trace_marks_what_it_ran_on(pool: PgPool) {
    let Some(w) = watched(&pool, module_path!()).await else {
        return;
    };
    let run = Uuid::new_v4();
    insert(
        &w,
        &[
            sent(
                &w.project,
                run,
                SpanKind::Agent,
                "2.0.0",
                at("2026-11-04T11:00:01Z"),
            ),
            SpanRow {
                release: "2.0.0".into(),
                ..call(
                    &w.project,
                    run,
                    "claude-sonnet-5-5",
                    &[
                        ("gen_ai.prompt.name", "triage"),
                        ("gen_ai.prompt.version", "3"),
                    ],
                    at("2026-11-04T11:00:00Z"),
                )
            },
        ],
    )
    .await;
    tick(&w, &pool, at("2026-11-04T11:01:00Z")).await;
    let kinds: Vec<String> = stored(&pool, &w.project)
        .await
        .into_iter()
        .map(|row| row.1)
        .collect();
    assert_eq!(kinds, ["model", "prompt", "release"]);
}

/// ⚠ Another exporter's trace, received forty minutes before the pass and held
/// for its parent, lands since the last pass: it is read by its landed trace,
/// and marked by its own first trace, though its receipt is long behind the
/// window. One received as long ago that landed before is not read again.
#[sqlx::test]
async fn another_exporters_trace_landed_late_is_marked_by_its_own_first_trace(pool: PgPool) {
    let Some(w) = watched(&pool, module_path!()).await else {
        return;
    };
    let now = at("2026-11-05T12:00:00Z");
    let (landed, earlier) = (Uuid::new_v4(), Uuid::new_v4());
    insert(
        &w,
        &[
            sent(
                &w.project,
                landed,
                SpanKind::Agent,
                "3.1.0",
                now - TimeDelta::minutes(40),
            ),
            sent(
                &w.project,
                earlier,
                SpanKind::Agent,
                "3.0.9",
                now - TimeDelta::minutes(40),
            ),
        ],
    )
    .await;
    for (run, span, landed_at) in [
        (landed, "11", now - TimeDelta::minutes(1)),
        (earlier, "22", now - TimeDelta::minutes(30)),
    ] {
        sqlx::query(
            "INSERT INTO telemetry.held_spans
                 (project_id, trace_id, span_id, parent_span_id, received_at, agent, landed_at)
             VALUES ($1, $2, $3, '', $4, $5, $6)",
        )
        .bind(&w.project)
        .bind(run.simple().to_string())
        .bind(span.repeat(8))
        .bind(now - TimeDelta::minutes(40))
        .bind(AGENT)
        .bind(landed_at)
        .execute(&pool)
        .await
        .expect("a landed span");
    }
    tick(&w, &pool, now).await;
    assert_eq!(
        stored(&pool, &w.project).await,
        [row("production", "release", None, "3.1.0", landed)]
    );
}

/// ⚠ After an outage, each pass reads on from the last, an hour at a time,
/// so a release carried only while the loop was down is still marked.
#[sqlx::test]
async fn a_pass_after_an_outage_goes_on_from_the_last(pool: PgPool) {
    let Some(w) = watched(&pool, module_path!()).await else {
        return;
    };
    let down = at("2026-11-06T08:00:00Z");
    let (during, later) = (Uuid::new_v4(), Uuid::new_v4());
    insert(
        &w,
        &[
            sent(
                &w.project,
                during,
                SpanKind::Agent,
                "a",
                down + TimeDelta::minutes(30),
            ),
            sent(
                &w.project,
                later,
                SpanKind::Agent,
                "b",
                down + TimeDelta::hours(3),
            ),
        ],
    )
    .await;
    last_pass(&pool, down).await;
    let now = down + TimeDelta::hours(4);
    changes::tick_at(&w.state, now).await.expect("a tick");
    assert_eq!(
        stored(&pool, &w.project).await,
        [row("production", "release", None, "a", during)],
        "the first hour after the last pass"
    );
    for _ in 0..4 {
        changes::tick_at(&w.state, now).await.expect("a tick");
    }
    assert_eq!(
        stored(&pool, &w.project).await,
        [
            row("production", "release", None, "a", during),
            row("production", "release", None, "b", later),
        ]
    );
    let read_to: DateTime<Utc> =
        sqlx::query_scalar("SELECT read_to FROM telemetry.reads WHERE name = $1")
            .bind(changes::HEARTBEAT)
            .fetch_one(&pool)
            .await
            .expect("the last pass");
    assert_eq!(read_to, now, "caught up");
}

/// One replica's tick writes while another holds the loop's lock, none; a
/// project a move holds is left to the next tick, which writes it once.
#[sqlx::test]
async fn one_replica_writes_and_a_project_a_move_holds_is_left(pool: PgPool) {
    let Some(w) = watched(&pool, module_path!()).await else {
        return;
    };
    let run = Uuid::new_v4();
    insert(
        &w,
        &[sent(
            &w.project,
            run,
            SpanKind::Agent,
            "5.0",
            at("2026-11-07T10:00:00Z"),
        )],
    )
    .await;

    let mut other = pool.begin().await.expect("another replica");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('telemetry:changes', 0))")
        .execute(&mut *other)
        .await
        .expect("its lock");
    tick(&w, &pool, at("2026-11-07T10:01:00Z")).await;
    assert!(
        stored(&pool, &w.project).await.is_empty(),
        "another replica's tick"
    );
    other.rollback().await.expect("let go");

    let mut mover = pool.begin().await.expect("a move");
    project_move::lock_for_move(&mut mover, &w.project)
        .await
        .expect("the move's lock");
    tick(&w, &pool, at("2026-11-07T10:02:00Z")).await;
    assert!(
        stored(&pool, &w.project).await.is_empty(),
        "a project a move holds"
    );
    mover.rollback().await.expect("let go");

    tick(&w, &pool, at("2026-11-07T10:03:00Z")).await;
    assert_eq!(
        stored(&pool, &w.project).await,
        [row("production", "release", None, "5.0", run)]
    );
}

/// A change as a past pass wrote it, for `project`.
async fn kept(
    pool: &PgPool,
    project: &ProjectId,
    agent: &str,
    (kind, prompt, value): (&str, Option<&str>, &str),
    changed_at: DateTime<Utc>,
    seen_at: DateTime<Utc>,
    mark: &str,
) {
    sqlx::query(
        "INSERT INTO telemetry.changes
             (id, project_id, agent, environment, kind, prompt, value, identity, changed_at,
              trace_id, seen_at, mark)
         VALUES ($1, $2, $3, 'production', $4, $5, $6, $7, $8, replace($1::text, '-', ''),
                 $9, $10)",
    )
    .bind(Uuid::new_v4())
    .bind(project)
    .bind(agent)
    .bind(kind)
    .bind(prompt)
    .bind(value)
    .bind(format!("{:064x}", Uuid::new_v4().as_u128()))
    .bind(changed_at)
    .bind(seen_at)
    .bind(mark)
    .execute(pool)
    .await
    .expect("a change");
}

/// Past a cutoff, a change whose trace's day it passed is hidden, one no span
/// since carried goes, and the rest stay; a project's purge takes its own.
#[sqlx::test]
async fn the_purges_hide_a_change_whose_trace_went_and_delete_one_no_span_carries(pool: PgPool) {
    let Some(w) = watched(&pool, module_path!()).await else {
        return;
    };
    let cutoff = at("2026-10-01T00:00:00Z");
    let (before, after) = (cutoff - TimeDelta::days(1), cutoff + TimeDelta::days(1));
    kept(
        &pool,
        &w.project,
        AGENT,
        ("release", None, "gone"),
        before,
        before,
        "shown",
    )
    .await;
    kept(
        &pool,
        &w.project,
        AGENT,
        ("release", None, "carried"),
        before,
        after,
        "shown",
    )
    .await;
    kept(
        &pool,
        &w.project,
        AGENT,
        ("release", None, "new"),
        after,
        after,
        "capped",
    )
    .await;
    let mut conn = pool.acquire().await.expect("a connection");
    changes::purge_before(&mut conn, None, cutoff)
        .await
        .expect("a purge");
    let marks: Vec<(String, String)> = sqlx::query_as(
        "SELECT value, mark FROM telemetry.changes WHERE project_id = $1 ORDER BY value",
    )
    .bind(&w.project)
    .fetch_all(&pool)
    .await
    .expect("what is left");
    assert_eq!(
        marks,
        [
            ("carried".to_owned(), "hidden".to_owned()),
            ("new".to_owned(), "capped".to_owned())
        ]
    );

    changes::purge_before(
        &mut conn,
        Some(&[ProjectId::new()]),
        after + TimeDelta::days(1),
    )
    .await
    .expect("another project's purge");
    assert_eq!(
        stored(&pool, &w.project).await.len(),
        2,
        "another project's cutoff"
    );

    let mut lane = maintenance_scope(&w.state.db, TelemetryLane)
        .await
        .expect("the lane");
    db::purge_project(&mut lane, &w.project)
        .await
        .expect("the project's purge");
    lane.commit().await.expect("committed");
    assert!(stored(&pool, &w.project).await.is_empty());
}

/// The console's lane and `/v1`'s, as the binary mounts them.
fn lanes(w: &Watched) -> Router {
    let gate = ApiKeyGate {
        auth: w.state.auth.clone(),
        scope: ApiKeyScope::Read,
        refuse: refuse_as_problem,
    };
    router(w.state.clone()).nest(
        "/v1",
        v1_reads(w.state.clone())
            .route_layer(axum::middleware::from_fn_with_state(gate, require_api_key)),
    )
}

/// `uri` asked by the member through the console, or with `key` on `/v1`.
async fn ask(app: &Router, w: &Watched, uri: &str, key: Option<&str>) -> (StatusCode, Value) {
    let request = match key {
        None => Request::builder()
            .uri(format!("/internal/v1/telemetry{uri}"))
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", format!("Bearer tok_{MEMBER}"))
            .header("x-organization-id", w.organization.as_str())
            .header("x-project-id", w.project.as_str()),
        Some(key) => Request::builder()
            .uri(format!("/v1{uri}"))
            .header("authorization", format!("Bearer {key}")),
    }
    .body(Body::empty())
    .expect("a request");
    let response = app.clone().oneshot(request).await.expect("an answer");
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

fn values(answer: &Value) -> Vec<String> {
    answer["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .map(|change| change["value"].as_str().expect("a value").to_owned())
        .collect()
}

/// Both lanes answer the window's shown changes, the newest first, capped
/// ones saying so, narrowed by agent and environment, `more` past 500, and
/// none of another project's; `/v1` refuses a key without `read`.
#[sqlx::test]
async fn the_lanes_answer_the_newest_first_narrowed_and_held_to_the_project(pool: PgPool) {
    let Some(w) = watched(&pool, module_path!()).await else {
        return;
    };
    let now = Utc::now();
    let ago = |hours| now - TimeDelta::hours(hours);
    kept(
        &pool,
        &w.project,
        AGENT,
        ("release", None, "1.0"),
        ago(3),
        ago(3),
        "shown",
    )
    .await;
    kept(
        &pool,
        &w.project,
        AGENT,
        ("model", None, "gpt-5"),
        ago(2),
        ago(2),
        "capped",
    )
    .await;
    kept(
        &pool,
        &w.project,
        AGENT,
        ("prompt", Some("triage"), "3"),
        ago(1),
        ago(1),
        "shown",
    )
    .await;
    kept(
        &pool,
        &w.project,
        AGENT,
        ("release", None, "held"),
        ago(1),
        ago(1),
        "hidden",
    )
    .await;
    kept(
        &pool,
        &w.project,
        "nightly",
        ("release", None, "9.9"),
        ago(4),
        ago(4),
        "shown",
    )
    .await;
    kept(
        &pool,
        &w.project,
        AGENT,
        ("release", None, "old"),
        ago(240),
        ago(1),
        "shown",
    )
    .await;
    kept(
        &pool,
        &ProjectId::new(),
        AGENT,
        ("release", None, "theirs"),
        ago(1),
        ago(1),
        "shown",
    )
    .await;
    let app = lanes(&w);

    let (status, all) = ask(&app, &w, "/changes", None).await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(values(&all), ["3", "gpt-5", "1.0", "9.9"]);
    assert_eq!(all["days"], 7);
    assert_eq!(all["more"], false);
    assert_eq!(all["changes"][0]["prompt"], "triage");
    assert_eq!(all["changes"][0]["kind"], "prompt");
    assert_eq!(all["changes"][1]["capped"], true);
    assert!(
        all["changes"][2].get("prompt").is_none(),
        "a release names no prompt"
    );

    let (_, narrowed) = ask(&app, &w, "/changes?agent=nightly", None).await;
    assert_eq!(values(&narrowed), ["9.9"]);
    let (_, narrowed) = ask(&app, &w, "/changes?environment=staging", None).await;
    assert!(values(&narrowed).is_empty());
    let (_, wider) = ask(&app, &w, "/changes?days=30", None).await;
    assert_eq!(values(&wider), ["3", "gpt-5", "1.0", "9.9", "old"]);
    for refused in [
        "/changes?days=0",
        "/changes?days=1&days=2",
        "/changes?nonsense=1",
    ] {
        let (status, _) = ask(&app, &w, refused, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    }

    let (status, by_key) = ask(&app, &w, "/changes", Some(READ_KEY)).await;
    assert_eq!(status, StatusCode::OK, "{by_key}");
    assert_eq!(by_key, all);
    let (status, _) = ask(&app, &w, "/changes", Some(INGEST_KEY)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    sqlx::query(
        "INSERT INTO telemetry.changes
             (id, project_id, agent, environment, kind, value, identity, changed_at, trace_id,
              seen_at, mark)
         SELECT gen_random_uuid(), $1, $2, 'production', 'release', 'r' || n,
                lpad(to_hex(n), 64, '0'), $3, replace(gen_random_uuid()::text, '-', ''), $3,
                'shown'
           FROM generate_series(1, 500) AS n",
    )
    .bind(&w.project)
    .bind(AGENT)
    .bind(ago(5))
    .execute(&pool)
    .await
    .expect("five hundred more");
    let (_, full) = ask(&app, &w, "/changes", None).await;
    assert_eq!(full["more"], true);
    assert_eq!(full["changes"].as_array().map(Vec::len), Some(500));
}

/// The Agents page reads each listed agent's newest change of each kind.
#[sqlx::test]
async fn the_agents_page_reads_each_agents_newest_change_of_each_kind(pool: PgPool) {
    let Some(w) = watched(&pool, module_path!()).await else {
        return;
    };
    let now = Utc::now();
    let ago = |hours| now - TimeDelta::hours(hours);
    insert(
        &w,
        &[sent(
            &w.project,
            Uuid::new_v4(),
            SpanKind::Agent,
            "1.1",
            ago(1),
        )],
    )
    .await;
    kept(
        &pool,
        &w.project,
        AGENT,
        ("release", None, "1.0"),
        ago(5),
        ago(5),
        "shown",
    )
    .await;
    kept(
        &pool,
        &w.project,
        AGENT,
        ("release", None, "1.1"),
        ago(1),
        ago(1),
        "shown",
    )
    .await;
    kept(
        &pool,
        &w.project,
        AGENT,
        ("model", None, "gpt-5"),
        ago(2),
        ago(2),
        "shown",
    )
    .await;
    kept(
        &pool,
        &w.project,
        "unlisted",
        ("release", None, "0.1"),
        ago(1),
        ago(1),
        "shown",
    )
    .await;
    let (status, agents) = ask(&lanes(&w), &w, "/agents", None).await;
    assert_eq!(status, StatusCode::OK, "{agents}");
    assert_eq!(values(&agents), ["gpt-5", "1.1"]);
}
