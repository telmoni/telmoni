//! Each project's settings under row-level security, and auth's purges and a
//! transfer's move, which reach the rows they name alone, in ClickHouse too.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use uuid::Uuid;

use telmoni_shared::acting::ActingKey;
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::{organization_scope, project_scope};
use telmoni_shared::seam::Telemetry;
use telmoni_shared::{
    ApiKeyScope, ContentMode, OrganizationId, OrganizationRole, ProjectId, SpanKind,
};
use telmoni_telemetry::seam::TelemetrySeam;
use telmoni_telemetry::{AppState, db, hashing};

/// The module over `pool` connected as its own role, with nobody seated:
/// nothing here asks who is acting.
fn state(pool: &PgPool) -> Arc<AppState> {
    common::state(pool, common::AuthStub::new())
}

/// A settings row, written as the owner, as the content switch will.
async fn seed(
    pool: &PgPool,
    organization: &OrganizationId,
    project: &ProjectId,
    mode: ContentMode,
) {
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id, content_mode) \
         VALUES ($1, $2, $3)",
    )
    .bind(project)
    .bind(organization)
    .bind(mode)
    .execute(pool)
    .await
    .expect("seed a project's settings");
}

async fn mode(state: &AppState, project: &ProjectId) -> ContentMode {
    let mut tx = project_scope(&state.db, project)
        .await
        .expect("a project scope");
    let mode = db::content_mode(&mut tx, project)
        .await
        .expect("read the mode");
    tx.commit().await.expect("commit");
    mode
}

async fn organization_of(pool: &PgPool, project: &ProjectId) -> Option<String> {
    sqlx::query_scalar(
        "SELECT organization_id FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(project)
    .fetch_optional(pool)
    .await
    .expect("read a row as the owner")
}

/// ⚠ Content is off until an owner turns it on: a project nobody has set
/// anything on reads as `off`.
#[sqlx::test]
async fn a_project_with_no_settings_keeps_no_content(pool: PgPool) {
    let state = state(&pool);
    assert_eq!(mode(&state, &ProjectId::new()).await, ContentMode::Off);
}

/// ⚠ **A project's scope reads its own settings and no other's**, and an
/// organization's reads its own projects' and writes none of them.
#[sqlx::test]
async fn a_project_reads_its_own_settings_and_no_others(pool: PgPool) {
    let state = state(&pool);
    let (ours, theirs) = (OrganizationId::new(), OrganizationId::new());
    let (on, off) = (ProjectId::new(), ProjectId::new());
    seed(&pool, &ours, &on, ContentMode::On).await;
    seed(&pool, &theirs, &off, ContentMode::Off).await;

    assert_eq!(mode(&state, &on).await, ContentMode::On);
    assert_eq!(mode(&state, &off).await, ContentMode::Off);

    let mut tx = project_scope(&state.db, &on).await.expect("scope");
    let visible: Vec<String> =
        sqlx::query_scalar("SELECT project_id FROM telemetry.project_settings")
            .fetch_all(tx.conn())
            .await
            .expect("read under the scope");
    tx.commit().await.expect("commit");
    assert_eq!(
        visible,
        [on.to_string()],
        "a project's scope read another project's row"
    );

    let mut tx = organization_scope(&state.db, &ours).await.expect("scope");
    let visible: Vec<String> =
        sqlx::query_scalar("SELECT project_id FROM telemetry.project_settings")
            .fetch_all(tx.conn())
            .await
            .expect("read under the organization");
    let rewritten = sqlx::query(
        "UPDATE telemetry.project_settings SET content_mode = 'off' WHERE project_id = $1",
    )
    .bind(&on)
    .execute(tx.conn())
    .await
    .expect("an organization's write runs, and reaches nothing")
    .rows_affected();
    tx.commit().await.expect("commit");
    assert_eq!(
        visible,
        [on.to_string()],
        "an organization read another's project"
    );
    assert_eq!(
        rewritten, 0,
        "an organization's scope rewrote a project's settings"
    );
}

/// ⚠ **A write binds the project's organization too**, which
/// `organization_read` trusts: a project's scope alone writes nothing, and a
/// scope naming another organization cannot file the row under it. The
/// switch's write, under the project and its own organization, lands, and
/// heals a row a transfer's failed settling left under another.
#[sqlx::test]
async fn a_write_binds_the_project_and_its_own_organization(pool: PgPool) {
    let state = state(&pool);
    let (ours, theirs) = (OrganizationId::new(), OrganizationId::new());
    let project = ProjectId::new();

    // `set_content_mode` takes no scope without the organization, so the
    // policy is asked directly.
    let mut tx = project_scope(&state.db, &project).await.expect("scope");
    let alone = sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id, content_mode) \
         VALUES ($1, $2, 'on')",
    )
    .bind(&project)
    .bind(&ours)
    .execute(tx.conn())
    .await;
    tx.rollback().await.expect("roll back");
    assert!(alone.is_err(), "a project's scope alone wrote its settings");

    let mut tx = project_scope(&state.db, &project)
        .await
        .expect("scope")
        .bind_organization(&theirs)
        .await
        .expect("bind another organization");
    let elsewhere = db::set_content_mode(&mut tx, &project, &ours, ContentMode::On).await;
    tx.rollback().await.expect("roll back");
    assert!(
        elsewhere.is_err(),
        "a scope filed a project's settings under an organization it did not bind"
    );

    for left_under in [None, Some(&theirs)] {
        if let Some(other) = left_under {
            sqlx::query(
                "UPDATE telemetry.project_settings SET organization_id = $2 WHERE project_id = $1",
            )
            .bind(&project)
            .bind(other)
            .execute(&pool)
            .await
            .expect("leave the row under another organization, as the owner");
        }
        let mut tx = project_scope(&state.db, &project)
            .await
            .expect("scope")
            .bind_organization(&ours)
            .await
            .expect("bind its own organization");
        db::set_content_mode(&mut tx, &project, &ours, ContentMode::On)
            .await
            .expect("the project's write under its own organization");
        tx.commit().await.expect("commit");
        assert_eq!(mode(&state, &project).await, ContentMode::On);
        assert_eq!(
            organization_of(&pool, &project).await.as_deref(),
            Some(ours.as_str()),
            "the write did not file the row under its own organization"
        );
    }
}

/// The purges and a transfer's move reach the rows they name and no others, idempotently.
#[sqlx::test]
async fn the_purges_and_the_move_reach_only_what_they_name(pool: PgPool) {
    let Some(ch) = common::clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let seam = TelemetrySeam(common::state_with_stores(&pool, common::AuthStub::new()));
    let (leaving, staying) = (OrganizationId::new(), OrganizationId::new());
    let (moved, left, deleted, untouched) = (
        ProjectId::new(),
        ProjectId::new(),
        ProjectId::new(),
        ProjectId::new(),
    );
    seed(&pool, &leaving, &moved, ContentMode::Off).await;
    seed(&pool, &leaving, &left, ContentMode::Off).await;
    seed(&pool, &staying, &deleted, ContentMode::Off).await;
    seed(&pool, &staying, &untouched, ContentMode::Off).await;
    for project in [&moved, &left, &deleted, &untouched] {
        ch.store
            .insert_spans(&[common::span(project, SpanKind::Run, "nightly-report", 100)])
            .await
            .expect("insert a project's span");
    }

    seam.move_project(&moved, &staying).await.expect("move");
    assert_eq!(
        organization_of(&pool, &moved).await,
        Some(staying.to_string())
    );
    seam.move_project(&ProjectId::new(), &staying)
        .await
        .expect("a project with no settings moves with nothing to move");

    assert_eq!(seam.purge_organization(&leaving).await.expect("purge"), 1);
    assert_eq!(
        seam.purge_project(&OrganizationId::new(), &deleted)
            .await
            .expect("purge"),
        1
    );
    assert_eq!(seam.purge_organization(&leaving).await.expect("again"), 0);
    assert_eq!(
        seam.purge_project(&OrganizationId::new(), &deleted)
            .await
            .expect("again"),
        0
    );

    assert_eq!(organization_of(&pool, &left).await, None);
    assert_eq!(organization_of(&pool, &deleted).await, None);
    assert_eq!(
        organization_of(&pool, &moved).await,
        Some(staying.to_string()),
        "the old organization's purge took a project that had left it"
    );
    assert_eq!(
        organization_of(&pool, &untouched).await,
        Some(staying.to_string())
    );

    for gone in [&left, &deleted] {
        common::eventually("a purged project's spans and totals are hidden", || async {
            common::rows_of(&ch.store, gone).await == (0, 0)
        })
        .await;
    }
    for kept in [&moved, &untouched] {
        assert_eq!(
            common::rows_of(&ch.store, kept).await,
            (1, 1),
            "a purge took the rows of a project it did not name"
        );
    }
}

/// A project's first export files its settings row under the organization
/// that holds the project now — the row's own, or auth's answer — never the
/// one its key was resolved under before a transfer. A project gone since
/// refuses the export, and a move under way makes it one to retry.
#[sqlx::test]
async fn a_first_export_files_the_settings_row_where_the_project_is_now(pool: PgPool) {
    let (left, holds) = (OrganizationId::new(), OrganizationId::new());
    let auth = common::AuthStub::probing(&pool);
    let state = common::state(&pool, auth.clone());
    // The export's key, resolved while `left` held the project, before a
    // transfer to `holds` committed.
    let resolved = |project: &ProjectId| ActingKey {
        token_id: Uuid::new_v4(),
        organization_id: left.clone(),
        project_id: project.clone(),
        scopes: vec![ApiKeyScope::Ingest],
    };

    // Its row moved with it, and no key in it yet: filed where the row
    // stands, and auth not asked.
    let moved = ProjectId::new();
    auth.seat("user_owner", &holds, &moved, OrganizationRole::Owner, None);
    seed(&pool, &holds, &moved, ContentMode::Off).await;
    hashing::storage_for(&state, &resolved(&moved))
        .await
        .expect("its storage");
    assert_eq!(
        organization_of(&pool, &moved).await,
        Some(holds.to_string())
    );
    assert_eq!(auth.homes_asked(), 0);

    // No row yet: auth's answer, asked with the project held in place.
    let fresh = ProjectId::new();
    auth.seat("user_owner", &holds, &fresh, OrganizationRole::Owner, None);
    hashing::storage_for(&state, &resolved(&fresh))
        .await
        .expect("its storage");
    assert_eq!(
        organization_of(&pool, &fresh).await,
        Some(holds.to_string())
    );
    assert_eq!(auth.homes_asked(), 1);

    // Deleted since its key was resolved: refused as the key now is, and
    // nothing written.
    let gone = ProjectId::new();
    let refused = hashing::storage_for(&state, &resolved(&gone))
        .await
        .expect_err("a project auth no longer holds");
    let problem = refused.to_problem_details();
    assert_eq!(
        (problem.status, problem.type_uri.as_str()),
        (401, "/errors/auth/invalid-token"),
        "{refused}"
    );
    assert_eq!(organization_of(&pool, &gone).await, None);
    assert_eq!(
        auth.homes_held(),
        [1, 1],
        "where the project is asked without its project held in place"
    );

    // A move under way: answered as one an exporter tries again.
    let moving = ProjectId::new();
    auth.seat("user_owner", &holds, &moving, OrganizationRole::Owner, None);
    let mut mover = pool.begin().await.expect("a move's transaction");
    project_move::lock_for_move(&mut mover, &moving)
        .await
        .expect("the move lock");
    let refused = hashing::storage_for(&state, &resolved(&moving))
        .await
        .expect_err("kept out by a move");
    assert_eq!(refused.to_problem_details().status, 503, "{refused}");
    assert_eq!(organization_of(&pool, &moving).await, None);
    mover.rollback().await.expect("the move lets go");
}

/// A first export auth cannot place is a 503 an exporter retries, writing
/// nothing; once auth answers, the row is filed.
#[sqlx::test]
async fn a_first_export_auth_cannot_place_is_tried_again(pool: PgPool) {
    let holds = OrganizationId::new();
    let auth = common::AuthStub::new();
    let state = common::state(&pool, auth.clone());
    let project = ProjectId::new();
    auth.seat(
        "user_owner",
        &holds,
        &project,
        OrganizationRole::Owner,
        None,
    );
    let key = ActingKey {
        token_id: Uuid::new_v4(),
        organization_id: holds.clone(),
        project_id: project.clone(),
        scopes: vec![ApiKeyScope::Ingest],
    };

    for slow in [false, true] {
        auth.failing_homes(!slow).delaying_homes(if slow {
            Duration::from_secs(3)
        } else {
            Duration::ZERO
        });
        let started = std::time::Instant::now();
        let refused = hashing::storage_for(&state, &key)
            .await
            .expect_err("a project auth cannot place");
        let problem = refused.to_problem_details();
        assert_eq!(
            (problem.status, problem.type_uri.as_str()),
            (503, "/errors/auth/identity-unavailable"),
            "slow {slow}: {refused}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "slow {slow}: auth waited out past its second"
        );
        assert_eq!(organization_of(&pool, &project).await, None, "slow {slow}");
    }

    auth.delaying_homes(Duration::ZERO);
    hashing::storage_for(&state, &key)
        .await
        .expect("its storage once auth answers");
    assert_eq!(
        organization_of(&pool, &project).await,
        Some(holds.to_string())
    );
}
