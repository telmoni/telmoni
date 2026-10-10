//! The spans and their rollup, against a real ClickHouse: an insert as the
//! module's user, with no setting on it, lands in both tables; a read sees
//! only the projects it names, by the row policy and by its own `WHERE`
//! each on its own, and a read naming none fails; the module's user reaches
//! nothing else; a failed read's error quotes none of its values; the
//! nightly purge drops whole days and months past retention and nothing
//! newer.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use chrono::{Duration, TimeZone, Utc};

use common::{clickhouse_or_skip, received, span};
use telmoni_shared::{ProjectId, SpanKind, SpanStatus};
use telmoni_telemetry::retention;
use telmoni_telemetry::store::{PROJECTS_SETTING, Projects, SpanRow};

/// A row of the rollup as the first suite reads it: kind, tool, status and
/// model, then spans, units, input tokens and cache reads.
type RollupRow = (String, String, String, String, u64, u64, u64, u64);

/// The policy's setting for `projects`, written as the query module writes
/// it, for a read sent round the module.
fn setting(projects: &[&ProjectId]) -> String {
    serde_json::to_string(&projects.iter().map(|p| p.as_str()).collect::<Vec<_>>())
        .expect("ids serialize")
}

#[tokio::test]
async fn an_insert_with_no_setting_lands_in_spans_and_the_hourly_rollup() {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = ProjectId::new();
    let call = SpanRow {
        model: "claude-sonnet-5-5".into(),
        provider: "anthropic".into(),
        input_tokens: 1_200,
        output_tokens: 300,
        cache_read_input_tokens: 1_000,
        customer_id: "9f86d081884c7d659a2feaa0c55ad015".into(),
        conversation_id: "thread-42".into(),
        tags: vec![
            ("team".into(), "search".into()),
            ("tier".into(), "gold".into()),
        ],
        attributes: vec![
            ("gen_ai.response.id".into(), "resp_01".into()),
            (
                "gen_ai.response.finish_reasons".into(),
                r#"["stop"]"#.into(),
            ),
        ],
        ..span(&project, SpanKind::ModelCall, "chat", 1_500)
    };
    let run = SpanRow {
        bound_stall_after_s: Some(600),
        bound_max_steps: Some(40),
        bound_max_cost_usd: Some(2.5),
        sample_rate: Some(0.25),
        ..span(&project, SpanKind::Run, "nightly-report", 2_000)
    };
    let tool = SpanRow {
        status: SpanStatus::Error,
        error_type: "TimeoutError".into(),
        units: 2,
        ..span(&project, SpanKind::ToolCall, "search_docs", 40)
    };
    let step = span(&project, SpanKind::Step, "plan the reply", 10);
    ch.store
        .insert_spans(&[call.clone(), run.clone(), tool, step])
        .await
        .expect("the module's user inserts with no setting naming a project");

    let only = Projects::one(project.clone());
    let back: Vec<SpanRow> = ch
        .store
        .read(
            &only,
            "SELECT project_id, received_at, units, run_id, agent, trace_id, span_id, \
             parent_span_id, kind, name, started_at, ended_at, status, error_type, model, \
             provider, input_tokens, output_tokens, cache_read_input_tokens, \
             cache_write_input_tokens, customer_id, conversation_id, tags, bound_stall_after_s, \
             bound_max_steps, bound_max_cost_usd, sample_rate, attributes \
             FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)} AND kind IN {kinds:Array(String)} \
             ORDER BY kind",
        )
        .param("kinds", vec!["model_call", "run"])
        .fetch_all()
        .await
        .expect("read the spans back");
    assert_eq!(back, [call, run], "a span came back other than it went in");

    let rollup: Vec<RollupRow> = ch
        .store
        .read(
            &only,
            "SELECT kind, tool, status, model, sum(spans), sum(units), sum(input_tokens), \
             sum(cache_read_input_tokens) \
             FROM telemetry.spans_hourly WHERE project_id IN {projects:Array(String)} \
             GROUP BY kind, tool, status, model ORDER BY kind",
        )
        .fetch_all()
        .await
        .expect("read the rollup");
    assert_eq!(
        rollup,
        [
            (
                "model_call".into(),
                String::new(),
                "ok".into(),
                "claude-sonnet-5-5".into(),
                1,
                1,
                1_200,
                1_000
            ),
            (
                "run".into(),
                String::new(),
                "ok".into(),
                String::new(),
                1,
                1,
                0,
                0
            ),
            (
                "step".into(),
                String::new(),
                "ok".into(),
                String::new(),
                1,
                1,
                0,
                0
            ),
            (
                "tool_call".into(),
                "search_docs".into(),
                "error".into(),
                String::new(),
                1,
                2,
                0,
                0
            ),
        ],
        "the rollup keys a tool call by its name, and no other kind by its own"
    );

    // A t-digest answers in `Float32`, whatever it was fed. A closed set
    // binds as its text.
    let durations: Vec<f32> = ch
        .store
        .read(
            &only,
            "SELECT quantilesTDigestMerge(0.5, 0.9, 0.99)(duration_ms) \
             FROM telemetry.spans_hourly \
             WHERE project_id IN {projects:Array(String)} AND kind = {kind:String}",
        )
        .param("kind", SpanKind::ModelCall.to_string())
        .fetch_one()
        .await
        .expect("read the rollup's durations");
    assert_eq!(durations, [1_500.0, 1_500.0, 1_500.0]);
}

/// ⚠ **Two locks, each enough on its own.** The row policy holds the
/// module's user to the projects a read's setting names, whatever the SQL;
/// the read's own `WHERE` holds a user the policy does not, as the Compose
/// quickstart's one user is.
#[tokio::test]
async fn a_read_naming_one_project_sees_no_other() {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (mine, theirs) = (ProjectId::new(), ProjectId::new());
    ch.store
        .insert_spans(&[
            span(&mine, SpanKind::Run, "nightly-report", 2_000),
            span(&theirs, SpanKind::Run, "nightly-report", 2_000),
            span(&theirs, SpanKind::ToolCall, "search_docs", 30),
        ])
        .await
        .expect("insert both projects' spans");

    // The policy alone: SQL with no tenant in it, sent round the module.
    for table in ["telemetry.spans", "telemetry.spans_hourly"] {
        let seen: Vec<String> = ch
            .telemetry
            .query_raw(&format!("SELECT DISTINCT project_id FROM {table}"))
            .with_setting(PROJECTS_SETTING, setting(&[&mine]))
            .fetch_all()
            .await
            .expect("read one project");
        assert!(
            seen.iter().all(|p| *p == mine.to_string()) && !seen.is_empty(),
            "the policy let {table} answer another project: {seen:?}"
        );
    }

    // The query's own `WHERE` alone: the module's reads, as a user the policy
    // does not hold.
    let seen: Vec<String> = ch
        .unheld
        .read(
            &Projects::one(mine.clone()),
            "SELECT DISTINCT project_id FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_all()
        .await
        .expect("read one project as the migrator");
    assert_eq!(
        seen,
        [mine.to_string()],
        "a read's own WHERE let another project in"
    );

    let both = Projects::of([mine.clone(), theirs.clone()]).expect("two projects");
    let mut seen: Vec<String> = ch
        .store
        .read(
            &both,
            "SELECT DISTINCT project_id FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_all()
        .await
        .expect("read two projects");
    seen.sort();
    let mut expected = vec![mine.to_string(), theirs.to_string()];
    expected.sort();
    assert_eq!(seen, expected);

    // An id that tries to name a second one is one id, and names nothing.
    let smuggled = ProjectId::from_trusted(format!("{mine}\",\"{theirs}'],['{theirs}"));
    let none: u64 = ch
        .store
        .read(
            &Projects::one(smuggled),
            "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_one()
        .await
        .expect("read an id no project has");
    assert_eq!(none, 0);
}

/// ⚠ **A read that names no project fails; it never answers every
/// project's rows.** The query module cannot send one, so this one goes
/// round it, as a query written by hand would.
#[tokio::test]
async fn a_read_naming_no_project_fails() {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = ProjectId::new();
    ch.store
        .insert_spans(&[span(&project, SpanKind::Run, "nightly-report", 100)])
        .await
        .expect("insert a span");

    assert!(
        Projects::of(Vec::new()).is_none(),
        "the module built a read naming none"
    );
    // ClickHouse masks the setting's name in its error, as every quoted
    // literal, so the refusal is known by its code.
    for table in ["telemetry.spans", "telemetry.spans_hourly"] {
        let refused = ch
            .telemetry
            .query_raw(&format!("SELECT count() FROM {table}"))
            .fetch_one::<u64>()
            .await
            .expect_err("a read naming no project answered");
        assert!(
            refused.to_string().contains("UNKNOWN_SETTING"),
            "{table} refused for another reason: {refused}"
        );
    }
}

/// ⚠ **A failed read's error quotes none of its values.** ClickHouse masks
/// every quoted literal in its errors and in its own log
/// (`crates/telemetry/clickhouse/low-memory.xml`), so a customer's id a
/// read binds reaches neither, nor the client's log of the error.
#[tokio::test]
async fn a_failed_reads_error_quotes_none_of_its_values() {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let failed = ch
        .store
        .read(
            &Projects::one(ProjectId::new()),
            "SELECT toUInt64({customer:String}) FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)}",
        )
        .param("customer", "customer-7731")
        .fetch_all::<u64>()
        .await
        .expect_err("a customer's id parsed as a number");
    let error = failed.to_string();
    assert!(
        error.contains("CANNOT_PARSE_TEXT") && !error.contains("customer-7731"),
        "the error quoted the value it failed on: {error}"
    );
}

/// ⚠ **The module's user reads the two tables and writes spans, and nothing
/// more.** The view would answer it past the policy, since it reads as its
/// definer; the rollup is the view's to write; the ledger is the migrator's.
#[tokio::test]
async fn the_module_user_reaches_nothing_but_its_two_tables() {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    for read in [
        "SELECT count() FROM telemetry.spans_hourly_mv",
        "SELECT count() FROM telemetry.migrations",
    ] {
        let refused = ch
            .telemetry
            .query_raw(read)
            .with_setting(PROJECTS_SETTING, setting(&[&ProjectId::new()]))
            .fetch_one::<u64>()
            .await
            .expect_err("the module's user read past its grants");
        assert!(
            refused.to_string().contains("ACCESS_DENIED"),
            "{read} refused for another reason: {refused}"
        );
    }
    let forged = ch
        .telemetry
        .query_raw(
            "INSERT INTO telemetry.spans_hourly \
             (project_id, hour, agent, kind, model, provider, tool, status, spans, units) \
             VALUES ('forged', now(), '', 'run', '', '', '', 'ok', 1, 1000000)",
        )
        .execute()
        .await
        .expect_err("the module's user wrote the rollup");
    assert!(
        forged.to_string().contains("ACCESS_DENIED"),
        "the rollup's write refused for another reason: {forged}"
    );
}

/// The policy holds every user but the one the file ran as: the migrator
/// reads every project's rows with no setting, as the nightly purge must.
#[tokio::test]
async fn the_migrator_is_not_held_by_the_row_policy() {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (one, two) = (ProjectId::new(), ProjectId::new());
    ch.store
        .insert_spans(&[
            span(&one, SpanKind::Run, "nightly-report", 100),
            span(&two, SpanKind::Run, "nightly-report", 100),
        ])
        .await
        .expect("insert two projects' spans");
    let seen: u64 = ch
        .migrator
        .query_raw(
            "SELECT count() FROM telemetry.spans \
             WHERE project_id IN ({one:String}, {two:String})",
        )
        .param("one", one.as_str())
        .param("two", two.as_str())
        .fetch_one()
        .await
        .expect("the migrator reads with no setting");
    assert_eq!(seen, 2);
}

/// Days and months past retention go whole; the rows of every day and month
/// inside it stay. The day is decades back, so no other suite's rows are in it.
#[tokio::test]
async fn the_nightly_purge_drops_whole_days_and_months_past_retention() {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = ProjectId::new();
    let long_ago = Utc
        .with_ymd_and_hms(2001, 1, 2, 3, 0, 0)
        .single()
        .expect("a valid instant");
    let inside = Utc::now() - Duration::days(retention::SPAN_RETENTION_DAYS - 1);
    ch.store
        .insert_spans(&[
            received(&project, SpanKind::Run, "nightly-report", 100, long_ago),
            received(&project, SpanKind::Run, "nightly-report", 100, inside),
            span(&project, SpanKind::Run, "nightly-report", 100),
        ])
        .await
        .expect("insert spans of three days");

    let purged = retention::purge(&ch.migrator, Utc::now())
        .await
        .expect("the purge runs as the migrator");
    assert!(purged.span_days >= 1, "no day was dropped: {purged:?}");
    assert!(
        purged.rollup_months >= 1,
        "no month was dropped: {purged:?}"
    );

    let only = Projects::one(project);
    let kept: u64 = ch
        .store
        .read(
            &only,
            "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_one()
        .await
        .expect("count what is left");
    assert_eq!(
        kept, 2,
        "the purge took a day inside retention, or left one past it"
    );
    let months: Vec<u32> = ch
        .store
        .read(
            &only,
            "SELECT DISTINCT toYYYYMM(hour) FROM telemetry.spans_hourly \
             WHERE project_id IN {projects:Array(String)} ORDER BY 1",
        )
        .fetch_all()
        .await
        .expect("list what is left of the rollup");
    assert!(
        !months.contains(&200_101),
        "a month past retention is still there: {months:?}"
    );
    assert!(!months.is_empty(), "the purge took this month's totals");

    let again = retention::purge(&ch.migrator, Utc::now())
        .await
        .expect("a second purge runs");
    assert_eq!(
        again,
        retention::Purged::default(),
        "a second night found more to drop"
    );
}

/// A second run of the file over the store it made changes nothing and is
/// let through: the ledger holds its digest.
#[tokio::test]
async fn the_file_runs_again_over_the_store_it_made() {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let ran = telmoni_telemetry::schema::apply(&ch.migrator)
        .await
        .expect("the file runs again over its own store");
    assert!(ran >= 7, "{ran} statements ran");
    let digests: Vec<String> = ch
        .migrator
        .query_raw("SELECT DISTINCT digest FROM telemetry.migrations")
        .fetch_all()
        .await
        .expect("read the ledger");
    assert_eq!(
        digests,
        [telmoni_telemetry::schema::digest(
            &telmoni_telemetry::schema::statements(telmoni_telemetry::schema::MIGRATION)
        )]
    );
}
