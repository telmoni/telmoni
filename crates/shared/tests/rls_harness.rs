//! The handler suites run as a role RLS applies to — proved, and kept that way.

use sqlx::PgPool;
use telmoni_shared::test_util::{ServiceRole, project_root, service_pool};

/// The handler's pool is filtered by the policies and the test's pool is not,
/// on a table of its own so this tests the harness rather than a migration.
#[sqlx::test]
async fn the_handlers_pool_is_filtered_and_the_tests_pool_is_not(owner: PgPool) {
    sqlx::query("CREATE SCHEMA IF NOT EXISTS rls_harness")
        .execute(&owner)
        .await
        .expect("create the probe schema");
    for ddl in [
        "CREATE TABLE rls_harness.probe (organization_id TEXT NOT NULL)",
        "ALTER TABLE rls_harness.probe ENABLE ROW LEVEL SECURITY",
        "ALTER TABLE rls_harness.probe FORCE ROW LEVEL SECURITY",
        "CREATE POLICY tenant_isolation ON rls_harness.probe
             USING      (organization_id = current_setting('app.organization_id', true))
             WITH CHECK (organization_id = current_setting('app.organization_id', true))",
        "INSERT INTO rls_harness.probe (organization_id) VALUES ('org_harness')",
    ] {
        sqlx::query(ddl)
            .execute(&owner)
            .await
            .expect("build the probe table");
    }

    let handler = service_pool(&owner, ServiceRole::Auth);

    let (role, superuser, bypass): (String, bool, bool) = sqlx::query_as(
        "SELECT current_user::text, rolsuper, rolbypassrls
           FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(&handler)
    .await
    .expect("read the handler pool's own identity");
    assert_eq!(role, "auth", "the handler pool is not the service role");
    assert!(!superuser, "the handler pool is still a superuser");
    assert!(!bypass, "the handler pool still holds BYPASSRLS");

    // The probe's own grant, issued once the pool's first connection has
    // created the service roles.
    for grant in [
        "GRANT USAGE ON SCHEMA rls_harness TO auth",
        "GRANT SELECT ON rls_harness.probe TO auth",
    ] {
        sqlx::query(grant)
            .execute(&owner)
            .await
            .expect("grant the probe to the handler's role");
    }

    let mut tx = telmoni_shared::db::tenant_session::organization_scope(
        &handler,
        &telmoni_shared::types::OrganizationId::try_new("org_harness").expect("valid id"),
    )
    .await
    .expect("bind the organization");
    let scoped: i64 = sqlx::query_scalar("SELECT count(*) FROM rls_harness.probe")
        .fetch_one(&mut *tx)
        .await
        .expect("scoped read — a failure here is a missing grant, not a policy");
    assert_eq!(scoped, 1, "the handler cannot read its own tenant's row");
    tx.rollback().await.expect("rollback");

    let unscoped: i64 = sqlx::query_scalar("SELECT count(*) FROM rls_harness.probe")
        .fetch_one(&handler)
        .await
        .expect("unscoped read");
    assert_eq!(
        unscoped, 0,
        "the handler pool read a row with no organization bound — it is bypassing RLS, \
         and every tenancy assertion made through it is vacuous"
    );

    let as_owner: i64 = sqlx::query_scalar("SELECT count(*) FROM rls_harness.probe")
        .fetch_one(&owner)
        .await
        .expect("owner read");
    assert_eq!(
        as_owner, 1,
        "the test's own pool is filtered too — fixtures and assertions are meant to see \
         everything, the way an operator with psql does"
    );
}

/// Suites whose pool never reaches Postgres, so there is no RLS to observe. One reason each.
const NO_DATABASE: &[(&str, &str)] = &[(
    "telmoni/tests/health.rs",
    "the probes are exercised against never-connecting lazy pools; no statement \
     is ever issued",
)];

/// Every handler suite hands its router a service-role pool.
#[test]
fn every_handler_suite_hands_its_router_a_service_role_pool() {
    let crates = project_root().join("crates");
    let mut checked = 0usize;
    let mut offenders: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(&crates)
        .expect("crates/ is readable")
        .flatten()
    {
        let tests = entry.path().join("tests");
        if !tests.is_dir() {
            continue;
        }
        for test in std::fs::read_dir(&tests)
            .expect("a tests/ dir is readable")
            .flatten()
        {
            let path = test.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let rel = format!(
                "{}/tests/{}",
                entry.file_name().to_string_lossy(),
                path.file_name().expect("a file name").to_string_lossy()
            );
            if NO_DATABASE.iter().any(|(f, _)| *f == rel) {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("a test file is readable");
            let role_pool = |value: &str| {
                value.starts_with("service_pool(") || value.starts_with("unreachable_pool(")
            };
            // The shorthand `db,` is the binding of that name, which a suite
            // makes when something besides the field needs the pool too (the
            // issuer takes `db.clone()`). It passes only when every `let db =`
            // in the file is a service-role pool.
            let db_bindings: Vec<&str> = src
                .lines()
                .filter_map(|line| line.trim().strip_prefix("let db = "))
                .collect();
            let mut db_fields = 0usize;
            for line in src.lines().map(str::trim) {
                if line == "db," {
                    db_fields += 1;
                    checked += 1;
                    if db_bindings.is_empty() || !db_bindings.iter().all(|v| role_pool(v)) {
                        offenders.push(format!(
                            "{rel}: `db,` shorthand over a binding that is not a service-role \
                             pool (`let db = {}`)",
                            db_bindings.join(" / let db = ")
                        ));
                    }
                    continue;
                }
                let Some(value) = line.strip_prefix("db: ") else {
                    continue;
                };
                db_fields += 1;
                checked += 1;
                if !role_pool(value) {
                    offenders.push(format!("{rel}: db: {value}"));
                }
            }
            if db_fields == 0
                && (src.contains(concat!("AppState", " {"))
                    || src.contains(concat!("AppState", "{")))
            {
                offenders.push(format!(
                    "{rel}: builds an AppState with no `db: ` line this scan can read"
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these suites give their router a pool RLS does not apply to, so every tenancy \
         assertion they make is vacuous — pass `service_pool(&pool, ServiceRole::<Module>)`:\n  {}",
        offenders.join("\n  ")
    );
    assert!(
        checked > 0,
        "this test found no `db:` field to check — the scan has drifted from how the \
         suites build their state, and a scan that matches nothing reports ok"
    );
}
