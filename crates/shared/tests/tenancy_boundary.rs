//! Posture tests asserting tenant isolation invariants across every table in
//! every service schema. RLS is the defence behind `project_scope`: a handler that
//! forgets the scope must still be unable to cross tenants.

#![expect(clippy::expect_used, reason = "test scaffolding")]

use sqlx::{PgPool, Row};
use telmoni_shared::db::tenant_session::organization_scope;
use telmoni_shared::test_util::tenancy::{database_url_or_skip, probe_schemas_migrated};
use telmoni_shared::types::OrganizationId;

/// Every service schema that owns database state; a new service adds its schema here.
const APP_SCHEMAS: &[&str] = &["auth", "notifications", "agent", "audit"];

/// The ephemeral NOLOGIN probe role used for the behavioural read test.
const PROBE_ROLE: &str = "telmoni_tenancy_boundary_probe";

/// A table's tenancy posture, as `information_schema` and `pg_class` report it.
#[derive(Debug)]
struct TenantTable {
    qualified: String,
    row_security: bool,
    force_row_security: bool,
    has_isolation_using: bool,
    has_isolation_check: bool,
    has_maintenance: bool,
    tenant_keys: Vec<String>,
    isolation_using: Option<String>,
}

/// Connect to the test database, or `None` if the environment provides none.
async fn pool_or_skip() -> Option<PgPool> {
    let url = database_url_or_skip(module_path!())?;
    match PgPool::connect(&url).await {
        Ok(pool) => {
            if !probe_schemas_migrated(&pool, APP_SCHEMAS).await {
                eprintln!("skipping tenancy_boundary: schemas not migrated");
                return None;
            }
            Some(pool)
        }
        Err(e) => {
            eprintln!("skipping tenancy_boundary: cannot connect to {url}: {e}");
            None
        }
    }
}

/// Every table carrying `project_id` or `organization_id`, with its RLS posture.
/// Both keys matter: a policy comparing the wrong GUC leaks or starves reads.
async fn tenant_tables(pool: &PgPool) -> Vec<TenantTable> {
    let rows = sqlx::query(
        r"
        SELECT
            n.nspname || '.' || c.relname                                   AS qualified,
            c.relrowsecurity                                                AS row_security,
            c.relforcerowsecurity                                           AS force_row_security,
            EXISTS (SELECT 1 FROM pg_policy p
                      WHERE p.polrelid = c.oid AND p.polname = 'tenant_isolation'
                        AND p.polqual IS NOT NULL)                          AS has_isolation_using,
            EXISTS (SELECT 1 FROM pg_policy p
                      WHERE p.polrelid = c.oid AND p.polname = 'tenant_isolation'
                        AND p.polwithcheck IS NOT NULL)                     AS has_isolation_check,
            EXISTS (SELECT 1 FROM pg_policy p
                      WHERE p.polrelid = c.oid AND p.polname = 'maintenance_access') AS has_maintenance,
            -- Every tenant column the table carries, so the check below can
            -- confirm the policy names one of them.
            ARRAY(SELECT a.attname::text FROM pg_attribute a
                   WHERE a.attrelid = c.oid
                     AND a.attname IN ('project_id', 'organization_id')
                     AND NOT a.attisdropped
                   ORDER BY a.attname)                                      AS tenant_keys,
            -- The policy's own text, so the assertion can hold the GUC it
            -- reads against the column it compares.
            (SELECT pg_get_expr(p.polqual, p.polrelid) FROM pg_policy p
                      WHERE p.polrelid = c.oid AND p.polname = 'tenant_isolation'
                      LIMIT 1)                                              AS isolation_using
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE c.relkind IN ('r', 'p')          -- ordinary, partitioned parents, and children
          AND n.nspname = ANY($1)
          AND EXISTS (
              SELECT 1 FROM pg_attribute a
              WHERE a.attrelid = c.oid
                AND a.attname IN ('project_id', 'organization_id')
                AND NOT a.attisdropped
          )
        ORDER BY qualified
        ",
    )
    .bind(APP_SCHEMAS)
    .fetch_all(pool)
    .await
    .expect("enumerate tenant tables");

    rows.into_iter()
        .map(|r| TenantTable {
            qualified: r.get("qualified"),
            row_security: r.get("row_security"),
            force_row_security: r.get("force_row_security"),
            has_isolation_using: r.get("has_isolation_using"),
            has_isolation_check: r.get("has_isolation_check"),
            has_maintenance: r.get("has_maintenance"),
            tenant_keys: r.get("tenant_keys"),
            isolation_using: r.get("isolation_using"),
        })
        .collect()
}

/// Every organization-scoped table is RLS-on, FORCE'd, with `tenant_isolation` and
/// `maintenance_access` — every other suite runs as a bypassing superuser.
#[tokio::test]
async fn every_project_scoped_table_is_forced_and_isolated() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let present: Vec<String> = sqlx::query_scalar(
        "SELECT nspname FROM pg_namespace WHERE nspname = ANY($1) ORDER BY nspname",
    )
    .bind(APP_SCHEMAS)
    .fetch_all(&pool)
    .await
    .expect("probe app schemas");
    if present.is_empty() {
        eprintln!(
            "skipping every_project_scoped_table_is_forced_and_isolated: no app schemas present"
        );
        return;
    }
    let missing: Vec<&&str> = APP_SCHEMAS
        .iter()
        .filter(|s| !present.iter().any(|p| p == **s))
        .collect();
    assert!(
        missing.is_empty(),
        "partial database — {} of {} app schemas are missing: {missing:?}.\n  \
         This suite asserts tenant isolation across EVERY service schema; over a \
         partial catalog it would pass while saying nothing about the missing ones.\n  \
         Run the workspace migrations (`cargo run -p telmoni -- migrate`) before testing.",
        missing.len(),
        APP_SCHEMAS.len(),
    );

    let tables = tenant_tables(&pool).await;

    assert!(
        !tables.is_empty(),
        "every app schema is present but no table carries an tenant column — \
         this indicates a broken catalog, not a green run"
    );

    let mut failures = Vec::new();
    for t in &tables {
        if !t.row_security {
            failures.push(format!("{}: rowsecurity is OFF", t.qualified));
        }
        if !t.force_row_security {
            failures.push(format!("{}: forcerowsecurity is OFF", t.qualified));
        }
        if !t.has_isolation_using {
            failures.push(format!(
                "{}: missing tenant_isolation USING policy",
                t.qualified
            ));
        }
        if !t.has_isolation_check {
            failures.push(format!(
                "{}: missing tenant_isolation WITH CHECK policy",
                t.qualified
            ));
        }
        if !t.has_maintenance {
            failures.push(format!(
                "{}: missing maintenance_access policy",
                t.qualified
            ));
        }
        if let Some(using) = &t.isolation_using {
            for key in ["organization_id", "project_id"] {
                if using.contains(&format!("app.{key}")) && !t.tenant_keys.iter().any(|k| k == key)
                {
                    failures.push(format!(
                        "{}: tenant_isolation reads app.{key} but the table has no {key} column \
                         (it has: {:?})",
                        t.qualified, t.tenant_keys,
                    ));
                }
                if using.contains(&format!("app.{key}")) && !using.contains(key) {
                    failures.push(format!(
                        "{}: tenant_isolation reads app.{key} but does not compare {key} \
                         to it — {using}. A crossed pair admits the wrong tenant's rows \
                         across the boundary.",
                        t.qualified,
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "RLS posture violations across tenant tables ({} failures):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// A table that is a PERSON's rather than any tenant's.
#[derive(Debug)]
struct PersonTable {
    qualified: String,
    row_security: bool,
    force_row_security: bool,
    isolation_using: Option<String>,
    isolation_check: Option<String>,
    has_maintenance: bool,
    has_lane_grant: bool,
}

/// Every table keyed on `user_id` and on no tenant column — the person's own
/// rows: identity, sessions, confirmation codes.
async fn person_tables(pool: &PgPool) -> Vec<PersonTable> {
    let rows = sqlx::query(
        r"
        SELECT
            n.nspname || '.' || c.relname                                   AS qualified,
            c.relrowsecurity                                                AS row_security,
            c.relforcerowsecurity                                           AS force_row_security,
            (SELECT pg_get_expr(p.polqual, p.polrelid) FROM pg_policy p
                      WHERE p.polrelid = c.oid AND p.polname = 'person_isolation'
                      LIMIT 1)                                              AS isolation_using,
            (SELECT pg_get_expr(p.polwithcheck, p.polrelid) FROM pg_policy p
                      WHERE p.polrelid = c.oid AND p.polname = 'person_isolation'
                      LIMIT 1)                                              AS isolation_check,
            EXISTS (SELECT 1 FROM pg_policy p
                      WHERE p.polrelid = c.oid AND p.polname = 'maintenance_access') AS has_maintenance,
            EXISTS (SELECT 1 FROM aclexplode(c.relacl) acl
                      JOIN pg_roles r ON r.oid = acl.grantee
                     WHERE r.rolname LIKE '%\_maintenance')                  AS has_lane_grant
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE c.relkind IN ('r', 'p')
          AND n.nspname = ANY($1)
          AND EXISTS (SELECT 1 FROM pg_attribute a
                       WHERE a.attrelid = c.oid AND a.attname = 'user_id' AND NOT a.attisdropped)
          AND NOT EXISTS (SELECT 1 FROM pg_attribute a
                           WHERE a.attrelid = c.oid
                             AND a.attname IN ('project_id', 'organization_id')
                             AND NOT a.attisdropped)
        ORDER BY qualified
        ",
    )
    .bind(APP_SCHEMAS)
    .fetch_all(pool)
    .await
    .expect("enumerate person tables");

    rows.into_iter()
        .map(|r| PersonTable {
            qualified: r.get("qualified"),
            row_security: r.get("row_security"),
            force_row_security: r.get("force_row_security"),
            isolation_using: r.get("isolation_using"),
            isolation_check: r.get("isolation_check"),
            has_maintenance: r.get("has_maintenance"),
            has_lane_grant: r.get("has_lane_grant"),
        })
        .collect()
}

/// ⚠ **The person's own tables are isolated as strictly as a tenant's.** They
/// carry no tenant column, so the suite above cannot see them. Each must be
/// RLS-on, FORCE'd, and hold a `person_isolation` policy comparing `user_id` to
/// `app.user_id` in both USING and WITH CHECK. `maintenance_access` is there exactly when a lane is
/// granted the table: without the grant the policy is dead text that reads as
/// access, and without the policy the grant reads zero rows. The codes hold
/// neither, because only their person ever touches them.
#[tokio::test]
async fn every_person_table_is_forced_and_isolated_on_the_person() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let tables = person_tables(&pool).await;
    for expected in [
        "auth.identities",
        "auth.sessions",
        "auth.confirmation_codes",
    ] {
        assert!(
            tables.iter().any(|t| t.qualified == expected),
            "{expected} is no longer a person table this suite can see — it gained a tenant \
             column, or lost `user_id`; either way the isolation it relies on changed"
        );
    }

    let mut failures = Vec::new();
    for t in &tables {
        if !t.row_security {
            failures.push(format!("{}: rowsecurity is OFF", t.qualified));
        }
        if !t.force_row_security {
            failures.push(format!("{}: forcerowsecurity is OFF", t.qualified));
        }
        match (t.has_lane_grant, t.has_maintenance) {
            (true, false) => failures.push(format!(
                "{}: a lane is granted the table but it has no maintenance_access policy",
                t.qualified
            )),
            (false, true) => failures.push(format!(
                "{}: maintenance_access policy with no lane grant behind it",
                t.qualified
            )),
            _ => {}
        }
        for (half, expr) in [
            ("USING", &t.isolation_using),
            ("WITH CHECK", &t.isolation_check),
        ] {
            match expr {
                None => failures.push(format!("{}: missing person_isolation {half}", t.qualified)),
                Some(e) if !(e.contains("user_id") && e.contains("app.user_id")) => {
                    failures.push(format!(
                        "{}: person_isolation {half} does not compare user_id to app.user_id — {e}",
                        t.qualified
                    ));
                }
                Some(_) => {}
            }
        }
    }
    assert!(
        failures.is_empty(),
        "RLS posture violations across person tables ({} failures):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// Every table keyed on a person AND a tenant, each with why its tenant may
/// read every person's rows.
const PERSON_IN_TENANT_TABLES: &[&str] = &[
    // The organization's roster: every member sees who else is on it.
    "auth.organization_members",
    // A project's roster: every seat holder sees who else holds one.
    "auth.project_members",
    // One handshake's CSRF token. `user_id` is checked against whoever
    // finishes the flow; the row itself holds only a hash.
    "notifications.oauth_states",
    // The agent's index. Not readable by the whole tenant: `tenant_isolation`
    // and `organization_level` admit only rows with no `user_id`, and a
    // person's own past exchanges are `author_access`'s, keyed on both.
    "agent.chunks",
    // A person's conversations and their messages. `tenant_isolation`
    // compares `user_id` to `app.user_id` as well as the organization, so
    // nobody else in it reads them.
    "agent.conversations",
    "agent.messages",
    // An erasure's fence: whom it erased, and when it reached the
    // organization. Every answer saved there reads whether one fell inside
    // its turn; the person is an id, and no lane serves the row.
    "agent.erasures",
];

/// ⚠ **A table keyed on both a person and a tenant is isolated only as far as
/// the tenant.** `tenant_isolation` lets anybody bound to the organization or
/// project read every person's rows in it — right for a roster, a leak for a
/// person's own settings. The two suites above each see such a table as theirs
/// and neither asks which it meant, so a new one fails here until it is
/// listed in [`PERSON_IN_TENANT_TABLES`] with the reason, or loses a key.
#[tokio::test]
async fn every_table_keyed_on_a_person_and_a_tenant_is_declared() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let found: Vec<String> = sqlx::query_scalar(
        r"
        SELECT n.nspname || '.' || c.relname
          FROM pg_class c
          JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE c.relkind IN ('r', 'p')
           AND NOT c.relispartition
           AND n.nspname = ANY($1)
           AND EXISTS (SELECT 1 FROM pg_attribute a
                        WHERE a.attrelid = c.oid AND a.attname = 'user_id' AND NOT a.attisdropped)
           AND EXISTS (SELECT 1 FROM pg_attribute a
                        WHERE a.attrelid = c.oid
                          AND a.attname IN ('project_id', 'organization_id')
                          AND NOT a.attisdropped)
         ORDER BY 1
        ",
    )
    .bind(APP_SCHEMAS)
    .fetch_all(&pool)
    .await
    .expect("enumerate person-in-tenant tables");

    let mut declared: Vec<String> = PERSON_IN_TENANT_TABLES
        .iter()
        .map(|t| (*t).to_owned())
        .collect();
    declared.sort();
    assert_eq!(
        found, declared,
        "the tables keyed on both `user_id` and a tenant column changed. A new one \
         is readable by its whole tenant: list it in PERSON_IN_TENANT_TABLES with why \
         that is right, or give it a person policy and drop the tenant key. A listed \
         one that is gone comes off the list."
    );
}

/// `APP_SCHEMAS` is comprehensive, or the loop above passes a new schema vacuously.
#[tokio::test]
async fn app_schemas_names_every_project_scoped_schema() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let system_schemas = &[
        "pg_catalog",
        "information_schema",
        "pg_toast",
        "public",
        "topology",
        "tiger",
        "tiger_data",
    ];

    let schemas_with_tenant_data: Vec<String> = sqlx::query_scalar(
        r"
        SELECT DISTINCT n.nspname
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        JOIN pg_attribute a ON a.attrelid = c.oid
        WHERE c.relkind IN ('r', 'p')
          AND NOT a.attisdropped
          AND a.attname IN ('project_id', 'organization_id')
          AND n.nspname != ALL($1)
        ORDER BY n.nspname
        ",
    )
    .bind(system_schemas)
    .fetch_all(&pool)
    .await
    .expect("find schemas with tenant data");

    let unlisted: Vec<String> = schemas_with_tenant_data
        .into_iter()
        .filter(|s| !APP_SCHEMAS.contains(&s.as_str()))
        .collect();

    assert!(
        unlisted.is_empty(),
        "schema(s) hold organization-scoped data but are absent from APP_SCHEMAS: {unlisted:?}.\n  \
         Every assertion in this file filters on that list, so these tables are \
         untested until their schema is added to APP_SCHEMAS."
    );
}

/// A probe bound to organization A cannot read B's row even given B's exact id —
/// the leak an injected body id would try to force.
#[tokio::test]
async fn a_foreign_row_is_invisible_even_by_exact_id() {
    let Some(pool) = pool_or_skip().await else {
        return;
    };
    let bootstrap = format!(
        "DO $$
         BEGIN
             IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{PROBE_ROLE}') THEN
                 CREATE ROLE {PROBE_ROLE} NOLOGIN;
             END IF;
         EXCEPTION WHEN duplicate_object THEN NULL;
         END $$;"
    );
    sqlx::query(&bootstrap)
        .execute(&pool)
        .await
        .expect("create the probe role");
    sqlx::query(&format!("GRANT USAGE ON SCHEMA auth TO {PROBE_ROLE}"))
        .execute(&pool)
        .await
        .expect("grant schema usage to the probe role");
    sqlx::query(&format!(
        "GRANT SELECT ON auth.organization_flags TO {PROBE_ROLE}"
    ))
    .execute(&pool)
    .await
    .expect("grant the probe table to the probe role");

    let run = uuid::Uuid::new_v4().simple().to_string();
    let (org_a, org_b) = (
        format!("org_boundary_a_{run}"),
        format!("org_boundary_b_{run}"),
    );
    for org in [&org_a, &org_b] {
        sqlx::query("INSERT INTO auth.organizations (external_id) VALUES ($1)")
            .bind(org)
            .execute(&pool)
            .await
            .expect("seed an organization");
    }

    let b_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO auth.organization_flags (organization_id, key, enabled, note, actor)
         VALUES ($1, 'members', false, 'boundary probe', 'operator') RETURNING id",
    )
    .bind(&org_b)
    .fetch_one(&pool)
    .await
    .expect("seed org B");
    sqlx::query(
        "INSERT INTO auth.organization_flags (organization_id, key, enabled, note, actor)
         VALUES ($1, 'members', false, 'boundary probe', 'operator')",
    )
    .bind(&org_a)
    .execute(&pool)
    .await
    .expect("seed org A");

    let mut tx = organization_scope(
        &pool,
        &OrganizationId::try_new(org_a.as_str()).expect("valid test org id"),
    )
    .await
    .expect("scope A");
    sqlx::query(&format!("SET LOCAL ROLE {PROBE_ROLE}"))
        .execute(&mut *tx)
        .await
        .expect("set probe role");
    let seen: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM auth.organization_flags WHERE id = $1")
            .bind(b_id)
            .fetch_optional(&mut *tx)
            .await
            .expect("probe read");
    assert!(
        seen.is_none(),
        "RLS breach: probe bound to org A was able to read org B's row by id: {seen:?}"
    );
    tx.rollback().await.expect("rollback");

    sqlx::query("DELETE FROM auth.organizations WHERE external_id = $1 OR external_id = $2")
        .bind(org_a)
        .bind(org_b)
        .execute(&pool)
        .await
        .expect("cleanup");
}
