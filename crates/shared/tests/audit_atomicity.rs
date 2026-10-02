//! A mutation and its audit row commit together or not at all. `emit_audit`

#![expect(clippy::expect_used, reason = "test scaffolding")]

use sqlx::PgPool;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::organization_scope;
use telmoni_shared::test_util::tenancy::database_url_or_skip;
use telmoni_shared::types::{AuditAction, OrganizationId, TelmoniResourceKind};
use uuid::Uuid;

const PROBE_ROLE: &str = "telmoni_audit_atomicity_probe";
const FIXTURE_LOCK: i64 = 0x5eed_a0d1;

async fn pool_or_skip() -> Option<PgPool> {
    let url = database_url_or_skip(module_path!())?;
    let pool = match PgPool::connect(&url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping audit_atomicity: {e}");
            return None;
        }
    };
    let ready: bool = sqlx::query_scalar(
        "SELECT to_regclass('audit.events') IS NOT NULL
            AND to_regclass('auth.organization_flags') IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("probe the schemas");
    if !ready {
        eprintln!("skipping audit_atomicity: audit or auth schema not migrated");
        return None;
    }

    let mut tx = pool.begin().await.expect("begin fixture tx");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(FIXTURE_LOCK)
        .execute(&mut *tx)
        .await
        .expect("fixture lock");
    for stmt in [
        format!(
            "DO $$ BEGIN
                IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{PROBE_ROLE}') THEN
                    CREATE ROLE {PROBE_ROLE} NOLOGIN;
                END IF;
            EXCEPTION WHEN duplicate_object THEN NULL; END $$;"
        ),
        format!("GRANT USAGE ON SCHEMA auth, audit TO {PROBE_ROLE}"),
        format!("GRANT SELECT, INSERT, DELETE ON auth.organization_flags TO {PROBE_ROLE}"),
        format!("GRANT SELECT, INSERT ON audit.events TO {PROBE_ROLE}"),
    ] {
        sqlx::query(&stmt)
            .execute(&mut *tx)
            .await
            .expect("fixture grant");
    }
    tx.commit().await.expect("commit fixture");
    Some(pool)
}

fn organization(tag: &str) -> (OrganizationId, OrganizationId) {
    let s = format!("org_atomicity_{tag}_{}", Uuid::now_v7().simple());
    (
        OrganizationId::try_new(&s).expect("valid org id"),
        OrganizationId::try_new(&s).expect("valid project id"),
    )
}

/// The organization a flag row names, as the owner, which bypasses RLS.
async fn seed_organization(pool: &PgPool, organization: &OrganizationId) {
    sqlx::query(
        "INSERT INTO auth.organizations (external_id, slug) VALUES ($1, 'org-' || md5($1))",
    )
    .bind(organization.as_str())
    .execute(pool)
    .await
    .expect("seed the organization");
}

/// The organization and every flag row on it, as the owner.
async fn drop_organization(pool: &PgPool, organization: &OrganizationId) {
    sqlx::query("DELETE FROM auth.organizations WHERE external_id = $1")
        .bind(organization.as_str())
        .execute(pool)
        .await
        .expect("cleanup");
}

async fn insert_flag(conn: &mut sqlx::PgConnection, organization: &OrganizationId, id: Uuid) {
    sqlx::query(
        "INSERT INTO auth.organization_flags (id, organization_id, key, enabled, note, actor, shard_key)
         VALUES ($1, $2, 'members', false, 'atomicity probe', 'operator', $3)",
    )
    .bind(id)
    .bind(organization.as_str())
    .bind(telmoni_shared::derive_shard_key(organization))
    .execute(conn)
    .await
    .expect("the row itself is accepted");
}

async fn flag_exists(pool: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM auth.organization_flags WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("count the row")
}

fn event<'a>(organization: &'a OrganizationId, resource_id: &'a str) -> AuditEvent<'a> {
    AuditEvent {
        organization_id: organization,
        in_project: None,
        actor: Actor::User("user_atomicity_probe"),
        action: AuditAction::Updated,
        resource_kind: TelmoniResourceKind::Organization,
        resource_id: Some(resource_id),
        request_id: None,
        ip_address: None,
        user_agent: None,
        metadata: None,
    }
}

#[tokio::test]
async fn a_refused_audit_row_takes_the_mutation_down_with_it() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (mine_org, _mine) = organization("mine");
    let (_other_org, other) = organization("other");
    let row = Uuid::now_v7();
    seed_organization(&pool, &mine_org).await;

    let mut tx = organization_scope(&pool, &mine_org).await.expect("scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("assume the probe role");
    insert_flag(&mut tx, &mine_org, row).await;

    let id = row.to_string();
    let refused = emit_audit(&mut tx, event(&other, &id)).await;
    assert!(
        refused.is_err(),
        "an audit row outside the scope must be refused"
    );
    let _ = tx.commit().await;

    assert!(
        !flag_exists(&pool, row).await,
        "the mutation committed without its audit row"
    );
    drop_organization(&pool, &mine_org).await;
}

#[tokio::test]
async fn a_row_and_its_audit_commit_together() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (mine_org, mine) = organization("both");
    let row = Uuid::now_v7();
    seed_organization(&pool, &mine_org).await;

    let mut tx = organization_scope(&pool, &mine_org).await.expect("scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("assume the probe role");
    insert_flag(&mut tx, &mine_org, row).await;
    let id = row.to_string();
    emit_audit(&mut tx, event(&mine, &id))
        .await
        .expect("an in-scope audit row lands");
    tx.commit().await.expect("commit");

    assert!(flag_exists(&pool, row).await);
    let audited: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM audit.events WHERE organization_id = $1 AND resource_id = $2)",
    )
    .bind(&mine)
    .bind(&id)
    .fetch_one(&pool)
    .await
    .expect("read the audit row");
    assert!(audited, "the audit row is missing after commit");

    drop_organization(&pool, &mine_org).await;
}
