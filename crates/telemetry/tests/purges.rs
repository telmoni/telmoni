//! The purges of a project's settings and views and of an organization's,
//! each project held alone as a move holds it, and the sweep that finishes a
//! purge cut short.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use common::AuthStub;
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::seam::Telemetry;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, SpanKind, TelmoniError};
use telmoni_telemetry::db::{self, TelemetryLane};
use telmoni_telemetry::seam::TelemetrySeam;
use telmoni_telemetry::sweep::{self, Swept};

const OWNER: &str = "user_owner";

/// A settings row of `project` filed under `organization`, and a view saved
/// on it.
async fn settings_and_a_view(pool: &PgPool, project: &ProjectId, organization: &OrganizationId) {
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id) VALUES ($1, $2)",
    )
    .bind(project)
    .bind(organization)
    .execute(pool)
    .await
    .expect("a settings row");
    sqlx::query(
        "INSERT INTO telemetry.trace_views (id, project_id, name, query, columns)
         VALUES ($1, $2, 'Errors', '{}', '{}')",
    )
    .bind(Uuid::now_v7())
    .bind(project)
    .execute(pool)
    .await
    .expect("a saved view");
}

/// The tables a purge empties in which the project has a row still.
async fn left_of(pool: &PgPool, project: &ProjectId) -> Vec<&'static str> {
    let mut left = Vec::new();
    for table in ["project_settings", "trace_views"] {
        let rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM telemetry.{table} WHERE project_id = $1"
        )))
        .bind(project)
        .fetch_one(pool)
        .await
        .expect("count the project's rows");
        if rows > 0 {
            left.push(table);
        }
    }
    left
}

/// The one session found waiting on `pid`'s locks, polled for as `what`.
async fn waiting_on(pool: &PgPool, pid: i32, what: &str) -> i32 {
    common::eventually(what, move || async move {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                             WHERE $1 = ANY (pg_blocking_pids(pid)))",
        )
        .bind(pid)
        .fetch_one(pool)
        .await
        .expect("ask who waits")
    })
    .await;
    let waiting: Vec<i32> = sqlx::query_scalar(
        "SELECT pid FROM pg_stat_activity WHERE $1 = ANY (pg_blocking_pids(pid))",
    )
    .bind(pid)
    .fetch_all(pool)
    .await
    .expect("the sessions waiting");
    assert_eq!(waiting.len(), 1, "{what}: {waiting:?} wait");
    waiting.first().copied().expect("the session waiting")
}

/// A transaction of `pool`'s holding `table` in `SHARE` mode, which every
/// write to the table waits on, and its session.
async fn holding(pool: &PgPool, table: &str) -> (sqlx::Transaction<'static, sqlx::Postgres>, i32) {
    let mut holder = pool.begin().await.expect("a holder's transaction");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .expect("a holder's session");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "LOCK TABLE {table} IN SHARE MODE"
    )))
    .execute(&mut *holder)
    .await
    .expect("hold the table");
    (holder, pid)
}

/// Those of `projects` an export could hold in place now.
async fn holdable(pool: &PgPool, projects: &[ProjectId]) -> Vec<ProjectId> {
    let mut held = Vec::new();
    for project in projects {
        let mut probe = pool.begin().await.expect("a probe's transaction");
        if project_move::try_hold_in_place(&mut probe, project)
            .await
            .expect("try the project")
        {
            held.push(project.clone());
        }
        probe.rollback().await.expect("the probe lets go");
    }
    held
}

/// A purge waits at most two seconds for a project an export holds in
/// place, even where the role sets no `lock_timeout`, then fails, the rows
/// standing.
#[sqlx::test]
async fn a_purge_waits_two_seconds_at_most_for_a_project_held_in_place(pool: PgPool) {
    let state = common::state(&pool, AuthStub::new());
    let project = ProjectId::new();
    settings_and_a_view(&pool, &project, &OrganizationId::new()).await;
    let mut in_place = pool.begin().await.expect("an export's transaction");
    assert!(
        project_move::try_hold_in_place(&mut in_place, &project)
            .await
            .expect("try the project")
    );
    let mut purger = maintenance_scope(&state.db, TelemetryLane)
        .await
        .expect("the lane");
    let started = std::time::Instant::now();
    let purged = tokio::time::timeout(
        Duration::from_secs(10),
        db::purge_project(&mut purger, &project),
    )
    .await
    .expect("the purge waited on past its lock's time");
    let waited = started.elapsed();
    let refused = purged.expect_err("a purge beside a project held in place");
    assert_eq!(
        refused
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("55P03"),
        "{refused}"
    );
    assert!(
        (Duration::from_secs(2)..Duration::from_secs(4)).contains(&waited),
        "{waited:?}"
    );
    purger.rollback().await.expect("the purge lets go");
    in_place.rollback().await.expect("the export ends");
    assert_eq!(
        left_of(&pool, &project).await,
        ["project_settings", "trace_views"]
    );
}

/// An organization's purge holds every project its settings rows name while
/// it deletes, and takes only the rows filed under the organization.
#[sqlx::test]
async fn an_organizations_purge_holds_every_project_its_rows_name(pool: PgPool) {
    let state = common::state(&pool, AuthStub::new());
    let (organization, elsewhere) = (OrganizationId::new(), OrganizationId::new());
    let ours = [ProjectId::new(), ProjectId::new()];
    for project in &ours {
        settings_and_a_view(&pool, project, &organization).await;
    }
    let theirs = ProjectId::new();
    settings_and_a_view(&pool, &theirs, &elsewhere).await;
    let (settings, settings_pid) = holding(&pool, "telemetry.project_settings").await;
    let mut purger = maintenance_scope(&state.db, TelemetryLane)
        .await
        .expect("the lane");
    let (purged, held) = tokio::join!(
        async {
            let purged = db::purge_organization(&mut purger, &organization)
                .await
                .expect("purged");
            purger.commit().await.expect("the purge commits");
            purged
        },
        async {
            waiting_on(
                &pool,
                settings_pid,
                "the purge waits to delete the settings rows",
            )
            .await;
            let held = holdable(&pool, &ours).await;
            settings.commit().await.expect("the settings let go");
            held
        },
    );
    assert!(held.is_empty(), "held in place beside the purge: {held:?}");
    assert_eq!(purged, 4, "a view and a settings row of each project");
    for project in &ours {
        assert!(left_of(&pool, project).await.is_empty(), "{project}");
    }
    assert_eq!(
        left_of(&pool, &theirs).await,
        ["project_settings", "trace_views"]
    );
}

/// The sweep finishes a purge cut short once auth no longer has the project,
/// and leaves a project auth still has alone, wherever its row is filed; a
/// pass auth or ClickHouse cannot answer changes nothing.
#[sqlx::test]
async fn a_projects_purge_cut_short_is_finished_by_the_sweep(pool: PgPool) {
    let Some(clickhouse) =
        common::clickhouse_or_skip("a_projects_purge_cut_short_is_finished_by_the_sweep").await
    else {
        return;
    };
    let auth = AuthStub::probing(&pool);
    let live = common::state_with_stores(&pool, auth.clone());
    // The same module over a ClickHouse that does not answer.
    let away = common::state(&pool, auth.clone());
    let organization = OrganizationId::new();
    let (gone, kept) = (ProjectId::new(), ProjectId::new());
    for project in [&gone, &kept] {
        auth.seat(OWNER, &organization, project, OrganizationRole::Owner, None);
        settings_and_a_view(&pool, project, &organization).await;
        clickhouse
            .store
            .insert_spans(&[common::span(project, SpanKind::Run, "hourly-sync", 100)])
            .await
            .expect("a span of the project's");
    }
    sqlx::query("UPDATE telemetry.project_settings SET organization_id = $2 WHERE project_id = $1")
        .bind(&kept)
        .bind(OrganizationId::new())
        .execute(&pool)
        .await
        .expect("the kept project's row filed under another organization");

    // Deleted, and its purge cut short by a ClickHouse that does not answer.
    auth.forget(&gone);
    TelemetrySeam(away.clone())
        .purge_project(&gone)
        .await
        .expect_err("a purge over a store that does not answer");
    let left = left_of(&pool, &gone).await;
    assert_eq!(left, ["project_settings", "trace_views"]);

    // A pass auth cannot answer, and one ClickHouse does not: nothing goes.
    auth.failing_homes(true);
    sweep::sweep(&live)
        .await
        .expect_err("a pass auth cannot answer");
    auth.failing_homes(false);
    let refused = sweep::sweep(&away)
        .await
        .expect_err("a pass over a store that does not answer");
    assert_eq!(
        refused.to_problem_details().type_uri,
        "/errors/telemetry/store-unavailable"
    );
    assert_eq!(left_of(&pool, &gone).await, left);
    assert_eq!(common::rows_of(&clickhouse.store, &gone).await, (1, 1));

    // A pass that can: the purge finished, and nothing of the other's gone.
    assert_eq!(
        sweep::sweep(&live).await.expect("a pass"),
        Swept { purged: 1, left: 0 }
    );
    assert!(left_of(&pool, &gone).await.is_empty());
    common::eventually("the deleted project's spans are marked deleted", || async {
        common::rows_of(&clickhouse.store, &gone).await == (0, 0)
    })
    .await;
    assert_eq!(
        left_of(&pool, &kept).await,
        ["project_settings", "trace_views"]
    );
    assert_eq!(common::rows_of(&clickhouse.store, &kept).await, (1, 1));
    assert_eq!(
        sweep::sweep(&live).await.expect("a pass"),
        Swept::default(),
        "nothing is left to finish"
    );
}

/// A purge the sweep cannot finish now is left for the next pass, and the
/// pass goes on to the rest.
#[sqlx::test]
async fn a_purge_the_sweep_cannot_finish_now_waits_for_the_next_pass(pool: PgPool) {
    let Some(_clickhouse) =
        common::clickhouse_or_skip("a_purge_the_sweep_cannot_finish_now_waits_for_the_next_pass")
            .await
    else {
        return;
    };
    let state = common::state_with_stores(&pool, AuthStub::probing(&pool));
    let organization = OrganizationId::new();
    for _ in 0..2 {
        settings_and_a_view(&pool, &ProjectId::new(), &organization).await;
    }
    let read: Vec<ProjectId> =
        sqlx::query_scalar("SELECT project_id FROM telemetry.project_settings ORDER BY project_id")
            .fetch_all(&pool)
            .await
            .expect("the rows in the order a pass reads them");
    assert_eq!(read.len(), 2, "{read:?}");
    let held = read.first().expect("the row read first");
    let other = read.last().expect("the row read after it");
    let mut in_place = pool.begin().await.expect("an export's transaction");
    assert!(
        project_move::try_hold_in_place(&mut in_place, held)
            .await
            .expect("hold the project in place")
    );
    assert_eq!(
        sweep::sweep(&state).await.expect("a pass"),
        Swept { purged: 1, left: 1 }
    );
    assert!(left_of(&pool, other).await.is_empty());
    assert_eq!(
        left_of(&pool, held).await,
        ["project_settings", "trace_views"]
    );
    in_place.rollback().await.expect("the export ends");
    assert_eq!(
        sweep::sweep(&state).await.expect("a pass"),
        Swept { purged: 1, left: 0 }
    );
    assert!(left_of(&pool, held).await.is_empty());
}

/// The sweep reads every settings row, a page at a time, asking auth once a
/// page.
#[sqlx::test]
async fn the_sweep_reads_every_page_of_settings_rows(pool: PgPool) {
    let Some(_clickhouse) =
        common::clickhouse_or_skip("the_sweep_reads_every_page_of_settings_rows").await
    else {
        return;
    };
    let auth = AuthStub::probing(&pool);
    let state = common::state_with_stores(&pool, auth.clone());
    let organization = OrganizationId::new();
    let projects: Vec<ProjectId> = (0..501).map(|_| ProjectId::new()).collect();
    for project in &projects {
        auth.seat(OWNER, &organization, project, OrganizationRole::Owner, None);
    }
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id)
         SELECT unnest($1::text[]), $2",
    )
    .bind(&projects)
    .bind(&organization)
    .execute(&pool)
    .await
    .expect("a settings row for each project");
    let ends: Vec<ProjectId> = sqlx::query_scalar(
        "(SELECT project_id FROM telemetry.project_settings ORDER BY project_id LIMIT 1)
         UNION ALL
         (SELECT project_id FROM telemetry.project_settings ORDER BY project_id DESC LIMIT 1)",
    )
    .fetch_all(&pool)
    .await
    .expect("the rows read first and last");
    for project in &ends {
        auth.forget(project);
    }
    let asked = auth.homes_asked();
    assert_eq!(
        sweep::sweep(&state).await.expect("a pass"),
        Swept { purged: 2, left: 0 }
    );
    assert_eq!(
        auth.homes_asked() - asked,
        2,
        "a page of 500, then one of 1"
    );
    for project in &ends {
        assert!(left_of(&pool, project).await.is_empty());
    }
    let standing: i64 = sqlx::query_scalar("SELECT count(*) FROM telemetry.project_settings")
        .fetch_one(&pool)
        .await
        .expect("count the settings rows");
    assert_eq!(standing, 499);
}

/// A database that stops answering ends the sweep's pass where it stopped.
#[sqlx::test]
async fn a_pass_that_loses_its_database_ends_there(pool: PgPool) {
    let Some(_clickhouse) =
        common::clickhouse_or_skip("a_pass_that_loses_its_database_ends_there").await
    else {
        return;
    };
    let auth = AuthStub::probing(&pool);
    let state = common::state_with_stores(&pool, auth.clone());
    let organization = OrganizationId::new();
    for _ in 0..2 {
        settings_and_a_view(&pool, &ProjectId::new(), &organization).await;
    }
    // The page read, auth's answer still to come when the pool closes.
    auth.delaying_homes(Duration::from_millis(500));
    let asked = auth.homes_asked();
    let (_logging, logs) = telmoni_shared::test_util::captured_logs();
    let (ended, ()) = tokio::join!(sweep::sweep(&state), async {
        common::eventually("the pass asks auth", || async {
            auth.homes_asked() > asked
        })
        .await;
        state.db.close().await;
    });
    assert!(
        matches!(ended, Err(TelmoniError::Database(sqlx::Error::PoolClosed))),
        "{ended:?}"
    );
    assert!(
        !logs
            .text()
            .contains("a deleted project's purge failed again"),
        "a project was tried as though the database had answered"
    );
}
