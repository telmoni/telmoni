//! RLS enforcement — the policies' actual filtering, pinned.
#![expect(clippy::expect_used, reason = "test scaffolding")]

use sqlx::PgPool;
use telmoni_shared::db::tenant_session::{
    MaintenanceLane, organization_scope, person_scope, project_scope,
};
use telmoni_shared::test_util::database_url_or_skip;
use telmoni_shared::test_util::lanes::enter;
use telmoni_shared::types::OrganizationId;
use telmoni_shared::{ProjectId, UserId};

/// A plain non-owner role: RLS applies, and it holds no maintenance membership.
const PROBE_ROLE: &str = "rls_probe";
/// Shaped like a real service role: a **member** of `auth_maintenance`
const MEMBER_PROBE_ROLE: &str = "rls_probe_maint";
/// Advisory-lock key serialising fixture construction across parallel tests.
const RLS_FIXTURE_LOCK: i64 = 0x7264_6C73_5F65_6E66;

/// Build the probe fixture. `None` means only "no database"; anything else panics,
/// because a fixture that quietly returns `None` is how broken coverage hides.
async fn pool_or_skip() -> Option<PgPool> {
    let url = database_url_or_skip(module_path!())?;
    let pool = PgPool::connect(&url)
        .await
        .expect("connect to test database");

    let mut tx = pool.begin().await.expect("begin fixture tx");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(RLS_FIXTURE_LOCK)
        .execute(&mut *tx)
        .await
        .expect("acquire fixture advisory lock");

    let feed_ready: bool =
        sqlx::query_scalar("SELECT to_regclass('notifications.feed') IS NOT NULL")
            .fetch_one(&mut *tx)
            .await
            .expect("probe for notifications.feed");
    assert!(
        feed_ready,
        "notifications.feed is missing — notifications isolation has zero test coverage without it"
    );

    let connections_ready: bool =
        sqlx::query_scalar("SELECT to_regclass('notifications.connections') IS NOT NULL")
            .fetch_one(&mut *tx)
            .await
            .expect("probe for notifications.connections");
    assert!(
        connections_ready,
        "notifications.connections is missing — the connector grants have zero isolation coverage without it"
    );

    let members_ready: bool =
        sqlx::query_scalar("SELECT to_regclass('auth.project_members') IS NOT NULL")
            .fetch_one(&mut *tx)
            .await
            .expect("probe for auth.project_members");
    assert!(
        members_ready,
        "auth.project_members is missing — membership isolation has zero test coverage without it"
    );

    let flags_ready: bool =
        sqlx::query_scalar("SELECT to_regclass('auth.organization_flags') IS NOT NULL")
            .fetch_one(&mut *tx)
            .await
            .expect("probe for auth.organization_flags");
    assert!(
        flags_ready,
        "auth.organization_flags is missing — this suite reads it as the organization-keyed \
         table, and is the only behavioural coverage the tenant-isolation policies have; it \
         must not be skipped"
    );

    for ddl in [
        format!(
            "DO $$ BEGIN
                IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{PROBE_ROLE}') THEN
                    CREATE ROLE {PROBE_ROLE} NOLOGIN;
                END IF;
                IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{MEMBER_PROBE_ROLE}') THEN
                    CREATE ROLE {MEMBER_PROBE_ROLE} NOLOGIN;
                END IF;
                IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'auth_maintenance') THEN
                    GRANT auth_maintenance TO {MEMBER_PROBE_ROLE};
                END IF;
            EXCEPTION WHEN duplicate_object THEN NULL; END $$;"
        ),
        format!("GRANT USAGE ON SCHEMA notifications, auth TO {PROBE_ROLE}, {MEMBER_PROBE_ROLE}"),
        format!(
            "GRANT SELECT, INSERT, DELETE ON notifications.feed TO {PROBE_ROLE}, {MEMBER_PROBE_ROLE}"
        ),
        format!(
            "GRANT SELECT, INSERT, UPDATE, DELETE ON notifications.connections, notifications.deliveries, \
             notifications.oauth_states, notifications.delivery_attempts TO {PROBE_ROLE}, {MEMBER_PROBE_ROLE}"
        ),
        format!(
            "GRANT SELECT, INSERT, UPDATE, DELETE ON auth.project_members TO {PROBE_ROLE}, {MEMBER_PROBE_ROLE}"
        ),
        format!(
            "GRANT SELECT, INSERT, UPDATE, DELETE ON auth.organization_flags TO {PROBE_ROLE}, {MEMBER_PROBE_ROLE}"
        ),
        format!(
            "GRANT SELECT, INSERT, DELETE ON auth.organization_members TO {PROBE_ROLE}, {MEMBER_PROBE_ROLE}"
        ),
        format!(
            "GRANT SELECT, INSERT, UPDATE ON auth.organizations TO {PROBE_ROLE}, {MEMBER_PROBE_ROLE}"
        ),
        format!("GRANT SELECT, INSERT ON auth.projects TO {PROBE_ROLE}, {MEMBER_PROBE_ROLE}"),
        format!(
            "GRANT SELECT, INSERT, DELETE ON auth.identities, auth.accounts, auth.sessions, auth.confirmation_codes TO {PROBE_ROLE}, {MEMBER_PROBE_ROLE}"
        ),
    ] {
        sqlx::query(&ddl)
            .execute(&mut *tx)
            .await
            .expect("probe role DDL");
    }

    tx.commit().await.expect("commit fixture");

    Some(pool)
}

/// Unique per-run org ids so parallel runs against the shared dev DB never collide.
fn oid(s: &str) -> OrganizationId {
    OrganizationId::try_new(s).expect("valid test org id")
}

fn pid(s: &str) -> ProjectId {
    ProjectId::try_new(s).expect("valid test project id")
}

fn uid(s: &str) -> UserId {
    UserId::try_new(s).expect("valid test user id")
}

fn tenant_ids() -> (String, String) {
    let run = uuid::Uuid::new_v4().simple().to_string();
    (
        format!("org_rlsprobe_a_{run}"),
        format!("org_rlsprobe_b_{run}"),
    )
}

/// Unique per-run PERSON ids, which never carry the organization prefix.
fn person_ids() -> (String, String) {
    let run = uuid::Uuid::new_v4().simple().to_string();
    (
        format!("user_rlsprobe_a_{run}"),
        format!("user_rlsprobe_b_{run}"),
    )
}

/// A person, as the code exchange would have recorded them; seeded as the
/// owner, which bypasses RLS.
async fn seed_person(pool: &PgPool, user: &str) {
    sqlx::query(
        "INSERT INTO auth.identities (user_id, email, email_verified) VALUES ($1, $2, true)",
    )
    .bind(user)
    .bind(format!("{user}@example.test"))
    .execute(pool)
    .await
    .expect("seed person");
}

#[tokio::test]
async fn policies_scope_probe_reads_and_fail_closed_without_an_org() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (org_a, org_b) = tenant_ids();
    seed_flags(&pool, &org_a, &org_b).await;

    let mut tx = organization_scope(&pool, &oid(&org_a))
        .await
        .expect("scope A");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let visible: Vec<String> = sqlx::query_scalar(
        "SELECT organization_id FROM auth.organization_flags WHERE organization_id = ANY($1)",
    )
    .bind(vec![org_a.clone(), org_b.clone()])
    .fetch_all(&mut *tx)
    .await
    .expect("scoped read");
    assert_eq!(visible, vec![org_a.clone()], "probe must see only org A");
    tx.rollback().await.expect("rollback");

    let mut tx = pool.begin().await.expect("begin");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.organization_flags WHERE organization_id = ANY($1)",
    )
    .bind(vec![org_a.clone(), org_b.clone()])
    .fetch_one(&mut *tx)
    .await
    .expect("unscoped read");
    assert_eq!(n, 0, "unset GUC must match nothing");

    sqlx::query("SELECT set_config('app.organization_id', '', true)")
        .execute(&mut *tx)
        .await
        .expect("bind empty GUC");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.organization_flags WHERE organization_id = ANY($1)",
    )
    .bind(vec![org_a.clone(), org_b.clone()])
    .fetch_one(&mut *tx)
    .await
    .expect("empty-GUC read");
    assert_eq!(n, 0, "the old empty-GUC wildcard must be dead");
    tx.rollback().await.expect("rollback");

    cleanup_flags(&pool, &org_a, &org_b).await;
}

/// One project's notifications, and nobody else's — proven as a NON-BYPASS role,
/// the layer below the handler's 403 that holds even if a role check is wrong.
async fn seed_feed(pool: &PgPool, org_a: &str, org_b: &str) {
    for org in [org_a, org_b] {
        sqlx::query(
            "INSERT INTO notifications.feed (project_id, organization_id, kind, title, body)
             VALUES ($1, $1, 'member_added', 'Test Title', 'Test Body')",
        )
        .bind(org)
        .execute(pool)
        .await
        .expect("seed feed row");
    }
}

#[tokio::test]
async fn feed_policy_scopes_reads_and_fails_closed() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (org_a, org_b) = tenant_ids();
    seed_feed(&pool, &org_a, &org_b).await;

    let mut tx = project_scope(&pool, &pid(&org_a)).await.expect("scope A");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let visible: Vec<String> =
        sqlx::query_scalar("SELECT project_id FROM notifications.feed WHERE project_id = ANY($1)")
            .bind(vec![org_a.clone(), org_b.clone()])
            .fetch_all(&mut *tx)
            .await
            .expect("scoped feed read");
    assert_eq!(
        visible,
        vec![org_a.clone()],
        "probe must see only org A feed"
    );
    tx.rollback().await.expect("rollback");

    let mut tx = project_scope(&pool, &pid(&org_a)).await.expect("scope A");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let denied = sqlx::query(
        "INSERT INTO notifications.feed (project_id, organization_id, kind, title, body)
         VALUES ($1, $1, 'member_added', 'Sneak', 'Sneak Body')",
    )
    .bind(&org_b)
    .execute(&mut *tx)
    .await
    .expect_err("cross-tenant feed INSERT must violate WITH CHECK");
    assert!(
        denied.to_string().contains("row-level security"),
        "expected RLS violation on cross-tenant feed insert, got: {denied}"
    );
    drop(tx);

    let mut tx = pool.begin().await.expect("begin");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications.feed WHERE project_id = ANY($1)")
            .bind(vec![org_a.clone(), org_b.clone()])
            .fetch_one(&mut *tx)
            .await
            .expect("unscoped feed read");
    assert_eq!(n, 0, "unset GUC must see zero feed rows");
    tx.rollback().await.expect("rollback");

    sqlx::query("DELETE FROM notifications.feed WHERE project_id = $1 OR project_id = $2")
        .bind(&org_a)
        .bind(&org_b)
        .execute(&pool)
        .await
        .expect("cleanup feed");
}

#[tokio::test]
async fn with_check_refuses_cross_org_writes() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (org_a, org_b) = tenant_ids();
    seed_flags(&pool, &org_a, &org_b).await;

    let mut tx = organization_scope(&pool, &oid(&org_a))
        .await
        .expect("scope A");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let denied = sqlx::query(
        "INSERT INTO auth.organization_flags (organization_id, key, enabled, note, actor)
         VALUES ($1, 'members', false, 'sneak', 'operator')",
    )
    .bind(&org_b)
    .execute(&mut *tx)
    .await
    .expect_err("cross-org INSERT must violate WITH CHECK");
    assert!(
        denied.to_string().contains("row-level security"),
        "expected an RLS violation, got: {denied}"
    );
    drop(tx); // aborted by the violation

    cleanup_flags(&pool, &org_a, &org_b).await;
}

#[tokio::test]
async fn maintenance_lane_spans_tenants() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (org_a, org_b) = tenant_ids();
    seed_flags(&pool, &org_a, &org_b).await;

    let mut tx = enter(&pool, MaintenanceLane::Auth)
        .await
        .expect("maintenance scope");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.organization_flags WHERE organization_id = ANY($1)",
    )
    .bind(vec![org_a.clone(), org_b.clone()])
    .fetch_one(&mut *tx)
    .await
    .expect("maintenance read");
    assert_eq!(n, 2, "maintenance lane must span tenants");

    // The lane reads across tenants; an override is written by the owner
    // (`make flag`), so the lane holds no INSERT to write one for any.
    let denied = sqlx::query(
        "INSERT INTO auth.organization_flags (organization_id, key, enabled, note, actor)
         VALUES ($1, 'members', false, 'sneak', 'operator')",
    )
    .bind(&org_b)
    .execute(&mut *tx)
    .await
    .expect_err("the auth lane must not write an override for any organization");
    assert!(
        denied.to_string().contains("permission denied"),
        "expected a privilege refusal, got: {denied}"
    );
    drop(tx);

    cleanup_flags(&pool, &org_a, &org_b).await;
}

/// ⚠ **A lane reaches its own service's schema and nothing else.** There was
/// one lane role for every service, granted in every schema, so any service
/// could `SET ROLE` into it and read a sibling's tables across all tenants.
#[tokio::test]
async fn a_maintenance_lane_cannot_reach_a_sibling_services_schema() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    for (lane, foreign) in [
        (MaintenanceLane::Auth, "notifications.connections"),
        (MaintenanceLane::Auth, "notifications.feed"),
        (MaintenanceLane::Notifications, "auth.api_tokens"),
        (MaintenanceLane::Notifications, "auth.organization_flags"),
        (MaintenanceLane::Notifications, "agent.chunks"),
        (MaintenanceLane::Auth, "agent.conversations"),
        // The agent indexes the chain through auth's seam, in auth's lane;
        // its own lane never reads it.
        (MaintenanceLane::Agent, "audit.events"),
        (MaintenanceLane::Agent, "auth.identities"),
        (MaintenanceLane::Agent, "notifications.deliveries"),
        (MaintenanceLane::Agent, "telemetry.project_settings"),
        (MaintenanceLane::Auth, "telemetry.project_settings"),
        (MaintenanceLane::Notifications, "telemetry.project_settings"),
        // Telemetry writes nothing a chain records, and reads no sibling's
        // table: what it needs of one comes through the seams.
        (MaintenanceLane::Telemetry, "audit.events"),
        (MaintenanceLane::Telemetry, "auth.projects"),
        (MaintenanceLane::Telemetry, "notifications.feed"),
        (MaintenanceLane::Telemetry, "agent.chunks"),
    ] {
        let mut tx = enter(&pool, lane).await.expect("enter the lane");
        let denied = sqlx::query(&format!("SELECT count(*) FROM {foreign}"))
            .execute(&mut *tx)
            .await
            .expect_err("a lane must not read a sibling's table");
        assert!(
            denied.to_string().contains("permission denied"),
            "{} read {foreign}: {denied}",
            lane.role()
        );
        drop(tx);
    }
}

/// Holding a lane role is not the same as being in the lane.
#[tokio::test]
async fn maintenance_membership_alone_does_not_widen_a_project_scope() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (org_a, org_b) = tenant_ids();
    seed_flags(&pool, &org_a, &org_b).await;

    let mut tx = organization_scope(&pool, &oid(&org_a))
        .await
        .expect("scope A");
    sqlx::query(&format!("SET LOCAL ROLE {MEMBER_PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set member probe role");
    let visible: Vec<String> = sqlx::query_scalar(
        "SELECT organization_id FROM auth.organization_flags WHERE organization_id = ANY($1) ORDER BY 1",
    )
    .bind(vec![org_a.clone(), org_b.clone()])
    .fetch_all(&mut *tx)
    .await
    .expect("scoped read");
    assert_eq!(
        visible,
        vec![org_a.clone()],
        "an auth_maintenance MEMBER that has not entered the lane must still be \
         scoped to its own org — seeing org B means the maintenance policy is \
         granting on membership again"
    );
    tx.rollback().await.expect("rollback");

    let mut tx = organization_scope(&pool, &oid(&org_a))
        .await
        .expect("scope A");
    sqlx::query(&format!("SET LOCAL ROLE {MEMBER_PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set member probe role");
    let denied = sqlx::query(
        "INSERT INTO auth.organization_flags (organization_id, key, enabled, note, actor)
         VALUES ($1, 'members', false, 'sneak', 'operator')",
    )
    .bind(&org_b)
    .execute(&mut *tx)
    .await
    .expect_err("cross-org INSERT by a mere member must violate WITH CHECK");
    assert!(
        denied.to_string().contains("row-level security"),
        "expected an RLS violation, got: {denied}"
    );
    drop(tx);

    cleanup_flags(&pool, &org_a, &org_b).await;
}

/// The FK chain a seat needs — the organization, its project (sharing the
/// organization's id as its own, which a project id may), and the person —
/// seeded as the owner, which bypasses RLS.
async fn seed_membership(pool: &PgPool, organization: &str, member: &str) {
    sqlx::query(
        "INSERT INTO auth.organizations (external_id, slug, name) VALUES ($1, 'org-' || md5($1), 'Acme')",
    )
    .bind(organization)
    .execute(pool)
    .await
    .expect("seed organization");
    seed_person(pool, member).await;
    sqlx::query(
        "INSERT INTO auth.projects (external_id, organization_id, name, slug)
         VALUES ($1, $1, 'Test Project', 'test-project')",
    )
    .bind(organization)
    .execute(pool)
    .await
    .expect("seed project");
    sqlx::query(
        "INSERT INTO auth.project_members (project_id, user_id, role, added_by)
         VALUES ($1, $2, 'admin', $2)",
    )
    .bind(organization)
    .bind(member)
    .execute(pool)
    .await
    .expect("seed membership");
}

async fn cleanup_membership(pool: &PgPool, organization: &str, people: &[&str]) {
    sqlx::query("DELETE FROM auth.project_members WHERE project_id = $1")
        .bind(organization)
        .execute(pool)
        .await
        .expect("cleanup members");
    sqlx::query("DELETE FROM auth.organization_members WHERE organization_id = $1")
        .bind(organization)
        .execute(pool)
        .await
        .expect("cleanup organization members");
    sqlx::query("DELETE FROM auth.projects WHERE external_id = $1")
        .bind(organization)
        .execute(pool)
        .await
        .expect("cleanup project");
    sqlx::query("DELETE FROM auth.organizations WHERE external_id = $1")
        .bind(organization)
        .execute(pool)
        .await
        .expect("cleanup organization");
    sqlx::query("DELETE FROM auth.identities WHERE user_id = ANY($1)")
        .bind(people)
        .execute(pool)
        .await
        .expect("cleanup people");
}

/// A member sees only the rows they were granted — proven as a NON-BYPASS role.
/// The owner may edit the row, a stranger cannot see it, and the member cannot promote themselves.
#[tokio::test]
async fn membership_policy_distinguishes_owner_and_stranger() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (organization, stranger_project) = tenant_ids();
    let (member, stranger) = person_ids();
    seed_membership(&pool, &organization, &member).await;
    seed_person(&pool, &stranger).await;

    let mut tx = project_scope(&pool, &pid(&organization))
        .await
        .expect("owner scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(&organization)
    .bind(&member)
    .fetch_one(&mut *tx)
    .await
    .expect("owner read");
    assert_eq!(count, 1, "owner must see their roster");
    tx.rollback().await.expect("rollback");

    let mut tx = project_scope(&pool, &pid(&stranger_project))
        .await
        .expect("stranger scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(&organization)
    .bind(&member)
    .fetch_one(&mut *tx)
    .await
    .expect("stranger read");
    assert_eq!(count, 0, "stranger must see zero rows of another roster");
    tx.rollback().await.expect("rollback");

    // The member reads the row that names them — and only reads it.
    let mut tx = person_scope(&pool, &uid(&member))
        .await
        .expect("member scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth.project_members WHERE user_id = $1")
            .bind(&member)
            .fetch_one(&mut *tx)
            .await
            .expect("member read");
    assert_eq!(count, 1, "a member must see the seat that names them");
    let promoted =
        sqlx::query("UPDATE auth.project_members SET role = 'member' WHERE user_id = $1")
            .bind(&member)
            .execute(&mut *tx)
            .await
            .expect("member update runs");
    assert_eq!(
        promoted.rows_affected(),
        0,
        "member_read is SELECT-only: being named in a row must not let you change it"
    );
    tx.rollback().await.expect("rollback");

    let mut tx = project_scope(&pool, &pid(&stranger_project))
        .await
        .expect("stranger scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let denied = sqlx::query(
        "INSERT INTO auth.project_members (project_id, user_id, role, added_by)
         VALUES ($1, $2, 'admin', $2)",
    )
    .bind(&organization)
    .bind(&stranger)
    .execute(&mut *tx)
    .await
    .expect_err("cross-tenant roster INSERT must violate WITH CHECK");
    assert!(
        denied.to_string().contains("row-level security"),
        "expected RLS violation on cross-tenant roster insert, got: {denied}"
    );
    drop(tx);

    cleanup_membership(&pool, &organization, &[&member, &stranger]).await;
}

/// ⚠ **A person's own rows, and the roster's view of people.** An identity, a
/// session or a confirmation code is readable under the person's own
/// `app.user_id` and nobody else's; an organization's scope sees the
/// identities of the people on its roster and of nobody else — the read the
/// members pages join on — and never a session or a code. Proven as a
/// NON-BYPASS role.
#[tokio::test]
async fn a_person_reads_only_their_own_rows_and_a_roster_only_its_people() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (organization, _) = tenant_ids();
    let (member, outsider) = person_ids();
    sqlx::query(
        "INSERT INTO auth.organizations (external_id, slug, name) VALUES ($1, 'org-' || md5($1), 'Acme')",
    )
    .bind(&organization)
    .execute(&pool)
    .await
    .expect("seed organization");
    for person in [&member, &outsider] {
        seed_person(&pool, person).await;
        sqlx::query("INSERT INTO auth.sessions (user_id, provider_sid) VALUES ($1, $1)")
            .bind(person)
            .execute(&pool)
            .await
            .expect("seed session");
        sqlx::query(
            "INSERT INTO auth.confirmation_codes (user_id, purpose, code_hash, expires_at)
             VALUES ($1, 'account_deletion', 'hash-' || $1, now() + interval '15 minutes')",
        )
        .bind(person)
        .execute(&pool)
        .await
        .expect("seed code");
        sqlx::query(
            "INSERT INTO auth.accounts (user_id, analytics_opt_in, deletion_requested_at)
             VALUES ($1, true, now())",
        )
        .bind(person)
        .execute(&pool)
        .await
        .expect("seed account");
    }
    sqlx::query(
        "INSERT INTO auth.organization_members (organization_id, user_id, role, added_by)
         VALUES ($1, $2, 'admin', $2)",
    )
    .bind(&organization)
    .bind(&member)
    .execute(&pool)
    .await
    .expect("seed roster row");
    let both = vec![member.clone(), outsider.clone()];

    async fn read(tx: &mut sqlx::PgConnection, table: &str, people: &[String]) -> Vec<String> {
        sqlx::query_scalar::<_, String>(&format!(
            "SELECT user_id FROM auth.{table} WHERE user_id = ANY($1) ORDER BY user_id"
        ))
        .bind(people)
        .fetch_all(tx)
        .await
        .expect("scoped read")
    }

    let mut tx = person_scope(&pool, &uid(&member)).await.expect("scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    assert_eq!(
        read(&mut tx, "identities", &both).await,
        vec![member.clone()]
    );
    assert_eq!(read(&mut tx, "sessions", &both).await, vec![member.clone()]);
    assert_eq!(
        read(&mut tx, "confirmation_codes", &both).await,
        vec![member.clone()],
        "a person must see their own codes and nobody else's"
    );
    assert_eq!(
        read(&mut tx, "accounts", &both).await,
        vec![member.clone()],
        "a person must see their own consent and deletion, and nobody else's"
    );
    let denied = sqlx::query("INSERT INTO auth.sessions (user_id) VALUES ($1)")
        .bind(&outsider)
        .execute(&mut *tx)
        .await
        .expect_err("a session written for somebody else must violate WITH CHECK");
    assert!(
        denied.to_string().contains("row-level security"),
        "{denied}"
    );
    drop(tx);

    let mut tx = organization_scope(&pool, &oid(&organization))
        .await
        .expect("scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    assert_eq!(
        read(&mut tx, "identities", &both).await,
        vec![member.clone()],
        "an organization must see the people on its roster and nobody else"
    );
    assert!(
        read(&mut tx, "sessions", &both).await.is_empty(),
        "an organization must never see anybody's sessions"
    );
    assert!(
        read(&mut tx, "confirmation_codes", &both).await.is_empty(),
        "an organization must never see anybody's codes"
    );
    assert!(
        read(&mut tx, "accounts", &both).await.is_empty(),
        "an organization must never see whether its members consented to analytics \
         or are deleting their accounts — that is why those facts left identities"
    );
    tx.rollback().await.expect("rollback");

    let mut tx = pool.begin().await.expect("begin");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    assert!(
        read(&mut tx, "identities", &both).await.is_empty(),
        "unset GUCs see nothing"
    );
    assert!(
        read(&mut tx, "sessions", &both).await.is_empty(),
        "unset GUCs see nothing"
    );
    assert!(
        read(&mut tx, "confirmation_codes", &both).await.is_empty(),
        "unset GUCs see nothing"
    );
    assert!(
        read(&mut tx, "accounts", &both).await.is_empty(),
        "unset GUCs see nothing"
    );
    tx.rollback().await.expect("rollback");

    sqlx::query("DELETE FROM auth.sessions WHERE user_id = ANY($1)")
        .bind(&both)
        .execute(&pool)
        .await
        .expect("cleanup sessions");
    sqlx::query("DELETE FROM auth.confirmation_codes WHERE user_id = ANY($1)")
        .bind(&both)
        .execute(&pool)
        .await
        .expect("cleanup codes");
    cleanup_membership(&pool, &organization, &[&member, &outsider]).await;
}

/// ⚠ **The tenant root is row-secured like every other table.** It was the one
/// table isolated only by each query's WHERE, so a forgotten predicate there
/// answered with every tenant. An organization's scope sees its own row; a
/// person sees the organizations they are in; a project sees the organization
/// that holds it; a stranger, an unbound transaction and a sibling's scope see
/// nothing, and nobody writes a row that is not their own organization's.
#[tokio::test]
async fn the_tenant_root_is_seen_only_by_its_own_scopes() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (org_a, org_b) = tenant_ids();
    let (member, stranger) = person_ids();
    seed_membership(&pool, &org_a, &member).await;
    sqlx::query("INSERT INTO auth.organization_members (organization_id, user_id, role, added_by) VALUES ($1, $2, 'member', $2)")
        .bind(&org_a)
        .bind(&member)
        .execute(&pool)
        .await
        .expect("seed roster row");
    sqlx::query(
        "INSERT INTO auth.organizations (external_id, slug, name) VALUES ($1, 'org-' || md5($1), 'Acme')",
    )
    .bind(&org_b)
    .execute(&pool)
    .await
    .expect("seed the sibling organization");
    seed_person(&pool, &stranger).await;
    let both = vec![org_a.clone(), org_b.clone()];

    async fn visible(tx: &mut sqlx::PgConnection, orgs: &[String]) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT external_id FROM auth.organizations WHERE external_id = ANY($1) ORDER BY 1",
        )
        .bind(orgs)
        .fetch_all(tx)
        .await
        .expect("scoped read")
    }

    let mut tx = organization_scope(&pool, &oid(&org_a))
        .await
        .expect("scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    assert_eq!(visible(&mut tx, &both).await, vec![org_a.clone()]);
    let renamed =
        sqlx::query("UPDATE auth.organizations SET name = 'moved' WHERE external_id = $1")
            .bind(&org_b)
            .execute(&mut *tx)
            .await
            .expect("the statement runs");
    assert_eq!(
        renamed.rows_affected(),
        0,
        "an organization renamed its sibling"
    );
    tx.rollback().await.expect("rollback");

    for (who, expected) in [(&member, vec![org_a.clone()]), (&stranger, vec![])] {
        let mut tx = person_scope(&pool, &uid(who)).await.expect("scope");
        sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
            .execute(&mut *tx)
            .await
            .expect("set probe role");
        assert_eq!(visible(&mut tx, &both).await, expected, "person {who}");
        let renamed =
            sqlx::query("UPDATE auth.organizations SET name = 'moved' WHERE external_id = $1")
                .bind(&org_a)
                .execute(&mut *tx)
                .await
                .expect("the statement runs");
        assert_eq!(
            renamed.rows_affected(),
            0,
            "member_read is SELECT-only: belonging to an organization must not let \
             a person scope rewrite it"
        );
        tx.rollback().await.expect("rollback");
    }

    let mut tx = project_scope(&pool, &pid(&org_a)).await.expect("scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    assert_eq!(
        visible(&mut tx, &both).await,
        vec![org_a.clone()],
        "a project must see the organization that holds it, and no other"
    );
    tx.rollback().await.expect("rollback");

    let mut tx = pool.begin().await.expect("begin");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    assert!(
        visible(&mut tx, &both).await.is_empty(),
        "unset GUCs see nothing"
    );
    let denied = sqlx::query(
        "INSERT INTO auth.organizations (external_id, slug, name) VALUES ($1, 'org-' || md5($1), 'Acme')",
    )
    .bind(format!("{org_b}_x"))
    .execute(&mut *tx)
    .await
    .expect_err("an organization created outside its own scope must violate WITH CHECK");
    assert!(
        denied.to_string().contains("row-level security"),
        "{denied}"
    );
    drop(tx);

    let mut tx = enter(&pool, MaintenanceLane::Auth)
        .await
        .expect("maintenance");
    assert_eq!(
        visible(&mut tx, &both).await,
        both,
        "the lane spans tenants"
    );
    tx.rollback().await.expect("rollback");

    sqlx::query("DELETE FROM auth.organizations WHERE external_id = $1")
        .bind(&org_b)
        .execute(&pool)
        .await
        .expect("cleanup sibling");
    cleanup_membership(&pool, &org_a, &[&member, &stranger]).await;
}

/// One organization's flag overrides, and nobody else's — proven as a NON-BYPASS
/// role. The global catalog has no tenant column; `organization_flags` carries RLS.
async fn seed_flags(pool: &PgPool, org_a: &str, org_b: &str) {
    for org in [org_a, org_b] {
        sqlx::query(
            "INSERT INTO auth.organizations (external_id, slug, name) VALUES ($1, 'org-' || md5($1), 'Acme')",
        )
        .bind(org)
        .execute(pool)
        .await
        .expect("seed organization for flag");
    }

    for org in [org_a, org_b] {
        sqlx::query(
            "INSERT INTO auth.organization_flags (organization_id, key, enabled, note, actor)
             VALUES ($1, 'custom_domains', true, 'test override', 'operator')",
        )
        .bind(org)
        .execute(pool)
        .await
        .expect("seed flag override");
    }
}

async fn cleanup_flags(pool: &PgPool, org_a: &str, org_b: &str) {
    sqlx::query(
        "DELETE FROM auth.organization_flags WHERE organization_id = $1 OR organization_id = $2",
    )
    .bind(org_a)
    .bind(org_b)
    .execute(pool)
    .await
    .expect("cleanup flags");
    sqlx::query("DELETE FROM auth.organizations WHERE external_id = $1 OR external_id = $2")
        .bind(org_a)
        .bind(org_b)
        .execute(pool)
        .await
        .expect("cleanup flag organizations");
}

#[tokio::test]
async fn flag_overrides_scope_reads_and_fail_closed() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (org_a, org_b) = tenant_ids();
    seed_flags(&pool, &org_a, &org_b).await;

    let mut tx = organization_scope(&pool, &oid(&org_a))
        .await
        .expect("scope A");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let visible: Vec<String> = sqlx::query_scalar(
        "SELECT organization_id FROM auth.organization_flags WHERE organization_id = ANY($1)",
    )
    .bind(vec![org_a.clone(), org_b.clone()])
    .fetch_all(&mut *tx)
    .await
    .expect("scoped flag read");
    assert_eq!(
        visible,
        vec![org_a.clone()],
        "probe must see only org A flags"
    );
    tx.rollback().await.expect("rollback");

    let mut tx = organization_scope(&pool, &oid(&org_a))
        .await
        .expect("scope A");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let denied = sqlx::query(
        "INSERT INTO auth.organization_flags (organization_id, key, enabled, note, actor)
         VALUES ($1, 'sso_enforced', true, 'sneak', 'operator')",
    )
    .bind(&org_b)
    .execute(&mut *tx)
    .await
    .expect_err("cross-tenant flag INSERT must violate WITH CHECK");
    assert!(
        denied.to_string().contains("row-level security"),
        "expected RLS violation on cross-tenant flag insert, got: {denied}"
    );
    drop(tx);

    let mut tx = pool.begin().await.expect("begin");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.organization_flags WHERE organization_id = ANY($1)",
    )
    .bind(vec![org_a.clone(), org_b.clone()])
    .fetch_one(&mut *tx)
    .await
    .expect("unscoped flag read");
    assert_eq!(n, 0, "unset GUC must see zero flag overrides");
    tx.rollback().await.expect("rollback");

    cleanup_flags(&pool, &org_a, &org_b).await;
}

/// ⚠ **THE GUC `acting_project` BINDS BEFORE READING `auth.organization_members`.**
/// It is the caller's `app.user_id` — the owner is a row now, so this one
/// read is how both an admin and the owner are recognised.
#[tokio::test]
async fn the_admin_membership_lookup_reads_under_the_person_guc() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (organization, _) = tenant_ids();
    let (admin, _) = person_ids();
    seed_membership(&pool, &organization, &admin).await;
    sqlx::query(
        "INSERT INTO auth.organization_members (organization_id, user_id, role, added_by)
         VALUES ($1, $2, 'admin', $2)",
    )
    .bind(&organization)
    .bind(&admin)
    .execute(&pool)
    .await
    .expect("seed organization membership");

    let lookup = "SELECT role::text FROM auth.organization_members
                   WHERE organization_id = $1 AND user_id = $2";

    let mut tx = person_scope(&pool, &uid(&admin))
        .await
        .expect("scope admin");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let found: Option<String> = sqlx::query_scalar(lookup)
        .bind(&organization)
        .bind(&admin)
        .fetch_optional(&mut *tx)
        .await
        .expect("membership read");
    assert_eq!(
        found.as_deref(),
        Some("admin"),
        "the admin's own membership row is invisible under app.user_id \
         — acting_project's fallback is dead again and every admin is locked out \
         of every project in their organization"
    );
    tx.rollback().await.expect("rollback");

    let mut tx = project_scope(&pool, &pid(&organization))
        .await
        .expect("scope project");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let found: Option<String> = sqlx::query_scalar(lookup)
        .bind(&organization)
        .bind(&admin)
        .fetch_optional(&mut *tx)
        .await
        .expect("membership read");
    assert_eq!(
        found, None,
        "app.project_id now admits organization_members rows — the policies moved, \
         and the comment in acting_project explaining this no longer holds"
    );
    tx.rollback().await.expect("rollback");

    cleanup_membership(&pool, &organization, &[&admin]).await;
}

/// The connector grant tables, under every scope they are read in.
async fn seed_connection(pool: &PgPool, organization: &str, project: &str) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO notifications.connections
            (id, project_id, organization_id, provider, external_workspace_id, channel_id,
             channel_name, target_ciphertext, target_nonce, key_version, wrapped_dek, scopes,
             installed_by)
         VALUES ($1, $2, $3, 'slack', 'T0', 'C0', '#c', '\\x00'::bytea, '\\x00'::bytea, 1, '\\x00'::bytea, 's', $3)",
    )
    .bind(id)
    .bind(project)
    .bind(organization)
    .execute(pool)
    .await
    .expect("seed connection");
    let delivery = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO notifications.deliveries
            (id, connection_id, project_id, organization_id, kind, subject, body)
         VALUES ($1, $2, $3, $4, 'member_added', 's', 'b')",
    )
    .bind(delivery)
    .bind(id)
    .bind(project)
    .bind(organization)
    .execute(pool)
    .await
    .expect("seed delivery");
    sqlx::query(
        "INSERT INTO notifications.delivery_attempts
            (id, delivery_id, project_id, organization_id, trigger, outcome, status_code,
             duration_ms)
         VALUES ($1, $2, $3, $4, 'scheduled', 'delivered', 200, 12)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(delivery)
    .bind(project)
    .bind(organization)
    .execute(pool)
    .await
    .expect("seed delivery attempt");
    id
}

#[tokio::test]
async fn connection_policies_scope_by_project_widen_by_organization_and_fail_closed() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let (org_a, org_b) = tenant_ids();
    let project_a1 = format!("{org_a}_t1");
    let project_a2 = format!("{org_a}_t2");
    let project_b = format!("{org_b}_t1");
    seed_connection(&pool, &org_a, &project_a1).await;
    seed_connection(&pool, &org_a, &project_a2).await;
    seed_connection(&pool, &org_b, &project_b).await;
    let all_projects = vec![project_a1.clone(), project_a2.clone(), project_b.clone()];

    async fn visible(tx: &mut sqlx::PgConnection, table: &str, projects: &[String]) -> Vec<String> {
        sqlx::query_scalar::<_, String>(&format!(
            "SELECT project_id FROM notifications.{table} WHERE project_id = ANY($1) ORDER BY project_id"
        ))
        .bind(projects)
        .fetch_all(tx)
        .await
        .expect("scoped read")
    }

    let mut tx = project_scope(&pool, &pid(&project_a1))
        .await
        .expect("scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    assert_eq!(
        visible(&mut tx, "connections", &all_projects).await,
        vec![project_a1.clone()]
    );
    assert_eq!(
        visible(&mut tx, "deliveries", &all_projects).await,
        vec![project_a1.clone()]
    );
    assert_eq!(
        visible(&mut tx, "delivery_attempts", &all_projects).await,
        vec![project_a1.clone()],
        "a project's delivery log shows its own sends and nobody else's"
    );
    tx.rollback().await.expect("rollback");

    let mut tx = organization_scope(&pool, &oid(&org_a))
        .await
        .expect("scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    assert_eq!(
        visible(&mut tx, "connections", &all_projects).await,
        vec![project_a1.clone(), project_a2.clone()]
    );
    assert_eq!(
        visible(&mut tx, "deliveries", &all_projects).await,
        vec![project_a1.clone(), project_a2.clone()]
    );
    assert!(
        visible(&mut tx, "delivery_attempts", &all_projects)
            .await
            .is_empty(),
        "the delivery log is read project by project; an organization binding \
         admits none of it"
    );
    sqlx::query(
        "INSERT INTO notifications.deliveries
            (id, connection_id, project_id, organization_id, kind, subject, body)
         SELECT $1, id, project_id, organization_id, 'member_added', 's', 'b'
           FROM notifications.connections WHERE project_id = $2",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(&project_a2)
    .execute(&mut *tx)
    .await
    .expect("an organization-scoped fan-out may enqueue for its own project");

    // The organization binding widens what the fan-out can SEE, never what it
    // can DO: a connection is its project's, and so is a delivery once queued.
    // Neither statement errors — the rows are simply not there to touch.
    for (table, statement) in [
        (
            "connections",
            "UPDATE notifications.connections SET channel_name = 'moved' WHERE project_id = $1",
        ),
        (
            "connections",
            "DELETE FROM notifications.connections WHERE project_id = $1",
        ),
        (
            "deliveries",
            "UPDATE notifications.deliveries SET status = 'failed' WHERE project_id = $1",
        ),
        (
            "deliveries",
            "DELETE FROM notifications.deliveries WHERE project_id = $1",
        ),
    ] {
        let touched = sqlx::query(statement)
            .bind(&project_a2)
            .execute(&mut *tx)
            .await
            .expect("the statement runs");
        assert_eq!(
            touched.rows_affected(),
            0,
            "an organization binding rewrote or removed a {table} row of one of its \
             own projects — the organization policies on {table} must never admit \
             an UPDATE or a DELETE"
        );
    }
    let denied = sqlx::query(
        "INSERT INTO notifications.oauth_states
            (state_hash, project_id, organization_id, user_id, provider, expires_at)
         VALUES ($1, $2, $3, 'user_probe', 'slack', now() + interval '10 minutes')",
    )
    .bind(uuid::Uuid::new_v4().simple().to_string())
    .bind(&project_a2)
    .bind(&org_a)
    .execute(&mut *tx)
    .await
    .expect_err("a handshake row is a project's; an organization binding must admit none");
    assert!(
        denied.to_string().contains("row-level security"),
        "{denied}"
    );
    drop(tx); // aborted by the violation

    let mut tx = project_scope(&pool, &pid(&project_a1))
        .await
        .expect("scope");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let denied = sqlx::query(
        "INSERT INTO notifications.connections
            (id, project_id, organization_id, provider, external_workspace_id, channel_id,
             channel_name, target_ciphertext, target_nonce, key_version, wrapped_dek, scopes,
             installed_by)
         VALUES ($1, $2, $3, 'slack', 'T9', 'C9', '#c', '\\x00'::bytea, '\\x00'::bytea, 1, '\\x00'::bytea, 's', $3)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(&project_b)
    .bind(&org_b)
    .execute(&mut *tx)
    .await
    .expect_err("cross-tenant connection INSERT must violate WITH CHECK");
    assert!(
        denied.to_string().contains("row-level security"),
        "{denied}"
    );
    drop(tx);

    let mut tx = pool.begin().await.expect("begin");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    assert!(
        visible(&mut tx, "connections", &all_projects)
            .await
            .is_empty(),
        "unset GUC must see nothing"
    );
    tx.rollback().await.expect("rollback");

    let mut tx = enter(&pool, MaintenanceLane::Notifications)
        .await
        .expect("maintenance");
    assert_eq!(
        visible(&mut tx, "connections", &all_projects).await.len(),
        3
    );
    tx.rollback().await.expect("rollback");

    sqlx::query(
        "DELETE FROM notifications.connections WHERE organization_id = $1 OR organization_id = $2",
    )
    .bind(&org_a)
    .bind(&org_b)
    .execute(&pool)
    .await
    .expect("cleanup connections");
}
