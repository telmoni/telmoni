//! Each project's settings, in Postgres, as the `telemetry` role under
//! row-level security: a project with none keeps no content, a project reads
//! its own settings and no other's, a write binds the project and its own
//! organization, and auth's purges and a transfer's move reach the rows they
//! name and no others.
#![expect(clippy::expect_used, reason = "test scaffolding")]

use std::sync::Arc;

use sqlx::PgPool;

use telmoni_shared::db::tenant_session::{organization_scope, project_scope};
use telmoni_shared::seam::Telemetry;
use telmoni_shared::test_util::service_pool;
use telmoni_shared::{ContentMode, OrganizationId, ProjectId};
use telmoni_telemetry::seam::TelemetrySeam;
use telmoni_telemetry::store::Store;
use telmoni_telemetry::{AppState, Config, db};

/// The module over `pool` connected as its own role. ClickHouse is named
/// and never dialled: nothing here reads a span.
fn state(pool: &PgPool) -> Arc<AppState> {
    Arc::new(AppState {
        db: service_pool(pool, "telemetry"),
        config: Config {
            database_url: "".into(),
            clickhouse_url: "http://telemetry:telemetry@127.0.0.1:1".into(),
        },
        store: Store::connect("http://telemetry:telemetry@127.0.0.1:1").expect("a ClickHouse URL"),
    })
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
    let (sealed, kept) = (ProjectId::new(), ProjectId::new());
    seed(&pool, &ours, &sealed, ContentMode::Sealed).await;
    seed(&pool, &theirs, &kept, ContentMode::On).await;

    assert_eq!(mode(&state, &sealed).await, ContentMode::Sealed);
    assert_eq!(mode(&state, &kept).await, ContentMode::On);

    let mut tx = project_scope(&state.db, &sealed).await.expect("scope");
    let visible: Vec<String> =
        sqlx::query_scalar("SELECT project_id FROM telemetry.project_settings")
            .fetch_all(tx.conn())
            .await
            .expect("read under the scope");
    tx.commit().await.expect("commit");
    assert_eq!(
        visible,
        [sealed.to_string()],
        "a project's scope read another project's row"
    );

    let mut tx = organization_scope(&state.db, &ours).await.expect("scope");
    let visible: Vec<String> =
        sqlx::query_scalar("SELECT project_id FROM telemetry.project_settings")
            .fetch_all(tx.conn())
            .await
            .expect("read under the organization");
    let rewritten = sqlx::query(
        "UPDATE telemetry.project_settings SET content_mode = 'on' WHERE project_id = $1",
    )
    .bind(&sealed)
    .execute(tx.conn())
    .await
    .expect("an organization's write runs, and reaches nothing")
    .rows_affected();
    tx.commit().await.expect("commit");
    assert_eq!(
        visible,
        [sealed.to_string()],
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

/// The purges reach the rows they name and no others, a transfer's move
/// takes a row out of its old organization's purge, and each is idempotent.
#[sqlx::test]
async fn the_purges_and_the_move_reach_only_what_they_name(pool: PgPool) {
    let seam = TelemetrySeam(state(&pool));
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

    seam.move_project(&moved, &staying).await.expect("move");
    assert_eq!(
        organization_of(&pool, &moved).await,
        Some(staying.to_string())
    );
    seam.move_project(&ProjectId::new(), &staying)
        .await
        .expect("a project with no settings moves with nothing to move");

    assert_eq!(seam.purge_organization(&leaving).await.expect("purge"), 1);
    assert_eq!(seam.purge_project(&deleted).await.expect("purge"), 1);
    assert_eq!(seam.purge_organization(&leaving).await.expect("again"), 0);
    assert_eq!(seam.purge_project(&deleted).await.expect("again"), 0);

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
}
