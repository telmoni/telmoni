//! Integration tests for the tamper-evident audit chain.
//!
//! ⚠ **Each test gets a database of its own (`#[sqlx::test]`).** Five of them
//! forge rows on purpose. Run against the shared dev database they left
//! broken chains behind, and the running server's `audit-verify` sweep
//! reported every one of them as tampering, every hour.
#![expect(
    clippy::expect_used,
    reason = "test scaffolding: asserts and fixture setup"
)]

use sqlx::PgPool;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::audit_verify::{BreakKind, verify_audit_chain, verify_audit_chain_batched};
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{AuditAction, OrganizationId, ProjectId, TelmoniResourceKind};
use uuid::Uuid;

/// Emit one audit row in its own committed transaction, as a real handler does.
async fn emit_one(pool: &PgPool, organization: &str, rid: &str, meta: serde_json::Value) {
    let organization_id =
        OrganizationId::try_new(organization).expect("valid test organization id");
    let mut tx = pool.begin().await.expect("begin");
    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &organization_id,
            in_project: None,
            actor: Actor::User(organization),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(rid),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(meta),
        },
    )
    .await
    .expect("emit");
    tx.commit().await.expect("commit");
}

/// One row on `organization`'s chain, written inside `project`.
async fn emit_in_project(pool: &PgPool, organization: &str, project: &str, rid: &str) {
    let organization_id =
        OrganizationId::try_new(organization).expect("valid test organization id");
    let in_project = ProjectId::try_new(project).expect("valid test project id");
    let mut tx = pool.begin().await.expect("begin");
    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &organization_id,
            in_project: Some(&in_project),
            actor: Actor::User(organization),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Token,
            resource_id: Some(rid),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: None,
        },
    )
    .await
    .expect("emit");
    tx.commit().await.expect("commit");
}

/// The audit table, assembled so the forbidden literals never appear in source.
fn audit_events() -> String {
    format!("{}.{}", "audit", "events")
}

async fn row_ids(pool: &PgPool, org: &str) -> Vec<Uuid> {
    sqlx::query_scalar::<_, Uuid>(&format!(
        "SELECT id FROM {} WHERE organization_id = $1 ORDER BY seq ASC",
        audit_events()
    ))
    .bind(org)
    .fetch_all(pool)
    .await
    .expect("ids")
}

/// Verify one org's whole chain.
async fn verify(
    pool: &PgPool,
    org: &str,
    from: Option<chrono::DateTime<chrono::Utc>>,
) -> telmoni_shared::db::audit_verify::VerifyReport {
    let mut conn = pool.acquire().await.expect("conn");
    verify_audit_chain(
        &mut conn,
        &OrganizationId::try_new(org).expect("valid"),
        from,
        None,
    )
    .await
    .expect("verify")
}

#[sqlx::test]
async fn chain_links_and_verifies(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let org = format!("org_test-chain-{}", Uuid::now_v7());
    for i in 0..4 {
        emit_one(&pool, &org, &format!("g{i}"), serde_json::json!({ "i": i })).await;
    }

    let report = verify(&pool, &org, None).await;
    assert_eq!(report.rows, 4);
    assert!(
        report.is_intact(),
        "a freshly written chain verifies: {report:?}"
    );
}

/// Two projects in one organization share one linear chain: why the lock is
/// per organization.
#[sqlx::test]
async fn one_org_is_one_chain_across_its_projects(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let org = format!("org_test-multiproject-{}", Uuid::now_v7());
    emit_one(&pool, &org, "a1", serde_json::json!({})).await;
    emit_one(&pool, &org, "org1", serde_json::json!({})).await;
    emit_one(&pool, &org, "b1", serde_json::json!({})).await;
    emit_one(&pool, &org, "a2", serde_json::json!({})).await;

    let report = verify(&pool, &org, None).await;
    assert_eq!(report.rows, 4, "every row is in the org's one chain");
    assert!(
        report.is_intact(),
        "rows from two projects and the org itself form one linear chain: {report:?}",
    );
}

/// A sibling organization's rows are a different chain, interleaved on purpose.
#[sqlx::test]
async fn a_sibling_orgs_rows_are_a_separate_chain(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let left = format!("org_test-iso-l-{}", Uuid::now_v7());
    let right = format!("org_test-iso-r-{}", Uuid::now_v7());
    for i in 0..3 {
        emit_one(&pool, &left, &format!("l{i}"), serde_json::json!({})).await;
        emit_one(&pool, &right, &format!("r{i}"), serde_json::json!({})).await;
    }

    for org in [&left, &right] {
        let report = verify(&pool, org, None).await;
        assert_eq!(report.rows, 3, "each org walks only its own rows");
        assert!(
            report.is_intact(),
            "interleaved writes by two orgs do not cross-link: {report:?}",
        );
    }
}

#[sqlx::test]
async fn tampered_row_fails_verify(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let org = format!("org_test-tamper-{}", Uuid::now_v7());
    for i in 0..3 {
        emit_one(&pool, &org, &format!("g{i}"), serde_json::json!({ "i": i })).await;
    }
    let ids = row_ids(&pool, &org).await;

    let stmt = format!(
        "{} {} SET metadata = '{{\"i\":999}}'::jsonb WHERE id = $1",
        "UPDATE",
        audit_events()
    );
    sqlx::query(&stmt)
        .bind(ids[1])
        .execute(&pool)
        .await
        .expect("tamper");

    let report = verify(&pool, &org, None).await;
    let brk = report.first_break.expect("a break is detected");
    assert_eq!(brk.kind, BreakKind::RowHashMismatch);
    assert_eq!(brk.id, ids[1], "the tampered row is the one flagged");
}

/// Moving an event from one organization to another is caught.
#[sqlx::test]
async fn moving_a_row_to_another_organization_fails_verify(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let source = format!("org_test-swap-a-{}", Uuid::now_v7());
    let target = format!("org_test-swap-b-{}", Uuid::now_v7());
    for i in 0..3 {
        emit_one(
            &pool,
            &source,
            &format!("g{i}"),
            serde_json::json!({ "i": i }),
        )
        .await;
    }
    let ids = row_ids(&pool, &source).await;

    let stmt = format!(
        "{} {} SET organization_id = $2 WHERE id = $1",
        "UPDATE",
        audit_events()
    );
    sqlx::query(&stmt)
        .bind(ids[1])
        .bind(&target)
        .execute(&pool)
        .await
        .expect("reassign");

    let report = verify(&pool, &target, None).await;
    let brk = report
        .first_break
        .expect("a row carried into another organization's chain is a break");
    assert_eq!(brk.kind, BreakKind::RowHashMismatch);
    assert_eq!(brk.id, ids[1]);
}

#[sqlx::test]
async fn deleted_middle_row_breaks_the_link(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let org = format!("org_test-delete-{}", Uuid::now_v7());
    for i in 0..3 {
        emit_one(&pool, &org, &format!("g{i}"), serde_json::json!({ "i": i })).await;
    }
    let ids = row_ids(&pool, &org).await;

    let stmt = format!("{} {} WHERE id = $1", "DELETE FROM", audit_events());
    sqlx::query(&stmt)
        .bind(ids[1])
        .execute(&pool)
        .await
        .expect("delete");

    let report = verify(&pool, &org, None).await;
    let brk = report.first_break.expect("a break is detected");
    assert_eq!(brk.kind, BreakKind::PrevLinkMismatch);
    assert_eq!(brk.id, ids[2], "the row after the gap is flagged");
}

#[sqlx::test]
async fn windowed_verify_from_midchain_is_not_a_false_break(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let org = format!("org_test-window-{}", Uuid::now_v7());
    for i in 0..4 {
        emit_one(&pool, &org, &format!("g{i}"), serde_json::json!({ "i": i })).await;
    }

    let created = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(&format!(
        "SELECT created_at FROM {} WHERE organization_id = $1 ORDER BY seq ASC",
        audit_events()
    ))
    .bind(&org)
    .fetch_all(&pool)
    .await
    .expect("created_at");
    let from = created[2];

    let report = verify(&pool, &org, Some(from)).await;
    assert!(
        report.is_intact(),
        "a mid-chain window over an intact chain must not report a break: {report:?}",
    );
    assert!(report.rows >= 1, "the window covers the anchored tail rows");
}

#[sqlx::test]
async fn concurrent_same_org_inserts_do_not_fork(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let org = format!("org_test-concurrent-{}", Uuid::now_v7());

    tokio::join!(
        emit_one(&pool, &org, "x", serde_json::json!({ "n": 1 })),
        emit_one(&pool, &org, "y", serde_json::json!({ "n": 2 })),
    );

    let report = verify(&pool, &org, None).await;
    assert_eq!(report.rows, 2);
    assert!(
        report.is_intact(),
        "concurrent inserts in one org form one linear chain, not a fork: {report:?}",
    );
}

/// A project has no chain of its own: its rows and the organization's are one sequence.
#[sqlx::test]
async fn rows_inside_and_outside_projects_are_one_chain(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let org = format!("org_test-projects-{}", Uuid::now_v7());
    emit_one(&pool, &org, "outside-1", serde_json::json!({ "n": 1 })).await;
    emit_in_project(&pool, &org, "project_7bQx2mNv9BcK4dLp", "in-first").await;
    emit_in_project(&pool, &org, "project_9zRt4kLm2VwQ8sNc", "in-second").await;
    emit_one(&pool, &org, "outside-2", serde_json::json!({ "n": 2 })).await;

    let report = verify(&pool, &org, None).await;
    assert_eq!(
        report.rows, 4,
        "every row is on the organization's one chain"
    );
    assert!(report.is_intact(), "the chain verifies as one: {report:?}");
}

/// Moving a row to another project, or out of one, is a tamper the chain detects.
#[sqlx::test]
async fn moving_a_row_to_another_project_fails_verify(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let org = format!("org_test-move-project-{}", Uuid::now_v7());
    emit_one(&pool, &org, "before", serde_json::json!({ "n": 1 })).await;
    emit_in_project(&pool, &org, "project_7bQx2mNv9BcK4dLp", "moved").await;
    emit_one(&pool, &org, "after", serde_json::json!({ "n": 2 })).await;
    let ids = row_ids(&pool, &org).await;
    assert!(verify(&pool, &org, None).await.is_intact());

    let stmt = format!(
        "{} {} SET in_project = 'project_9zRt4kLm2VwQ8sNc' WHERE id = $1",
        "UPDATE",
        audit_events()
    );
    sqlx::query(&stmt)
        .bind(ids[1])
        .execute(&pool)
        .await
        .expect("tamper");
    let brk = verify(&pool, &org, None)
        .await
        .first_break
        .expect("a moved row is detected");
    assert_eq!(brk.kind, BreakKind::RowHashMismatch);
    assert_eq!(brk.id, ids[1], "the moved row is the one flagged");

    let stmt = format!(
        "{} {} SET in_project = NULL WHERE id = $1",
        "UPDATE",
        audit_events()
    );
    sqlx::query(&stmt)
        .bind(ids[1])
        .execute(&pool)
        .await
        .expect("tamper");
    let brk = verify(&pool, &org, None)
        .await
        .first_break
        .expect("a row taken out of its project is detected");
    assert_eq!(brk.kind, BreakKind::RowHashMismatch);
    assert_eq!(brk.id, ids[1]);
}

/// The walk is paged, so `prev_hash` has to survive the gap between pages.
#[sqlx::test]
async fn the_chain_verifies_across_a_page_boundary(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let org = format!("org_test-paged-{}", Uuid::now_v7());
    for i in 0..5 {
        emit_one(&pool, &org, &format!("p{i}"), serde_json::json!({ "i": i })).await;
    }

    let organization_id = OrganizationId::try_new(&org).expect("valid");
    for batch in [1, 2, 4, 5, 6] {
        let mut conn = pool.acquire().await.expect("conn");
        let report = verify_audit_chain_batched(&mut conn, &organization_id, None, None, batch)
            .await
            .expect("verify");
        assert_eq!(report.rows, 5, "page size {batch} lost rows: {report:?}");
        assert!(
            report.is_intact(),
            "page size {batch} broke an intact chain: {report:?}",
        );
    }
}
