//! The runtime Postgres privilege matrix — what each service role may do, and
//! to whose tables. A widened grant fails the build.
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test scaffolding: asserts and fixture setup"
)]

use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use telmoni_shared::test_util::database_url_or_skip;

/// The grants under test: the bytes the migrator embeds, so they cannot drift.
const OBJECT_GRANTS_SQL: &str = include_str!("../../migrator/sql/object_grants.sql");

/// Runtime service role → the schema it owns the data in.
const SERVICE_ROLES: &[(&str, &str)] = &[
    ("auth", "auth"),
    ("notifications", "notifications"),
    ("agent", "agent"),
];

/// Every service's schema, so each role is checked against each sibling.
const CROSS_SCHEMAS: &[&str] = &["auth", "notifications", "agent"];

/// Roles the grants file names, plus `migrator`, which its default
/// privileges need, plus the lanes, which their modules' migrations grant on.
const REQUIRED_ROLES: &[&str] = &[
    "auth",
    "notifications",
    "agent",
    "migrator",
    "auth_maintenance",
    "notifications_maintenance",
    "agent_maintenance",
];

/// Each maintenance lane → its own schema, and whether it reaches `audit`.
/// The agent's does not: it reads the chain through auth's seam, in auth's
/// lane, and writes nothing to it.
const LANES: &[(&str, &str, bool)] = &[
    ("auth_maintenance", "auth", true),
    ("notifications_maintenance", "notifications", true),
    ("agent_maintenance", "agent", false),
];

/// ⚠ **A lane reaches its own service's schema, and no sibling's.** There was
/// one lane role for every service, holding privileges in every schema, so any
/// service could `SET ROLE` into it and read its siblings' tables across all
/// tenants.
#[tokio::test]
async fn each_maintenance_lane_reaches_its_own_schema_and_no_siblings() {
    let Some(pool) = harness().await else { return };
    let mut checks = 0usize;

    for &(lane, own, audits) in LANES {
        for &schema in CROSS_SCHEMAS {
            let usage = has_schema_privilege(&pool, lane, schema, "USAGE").await;
            if schema == own {
                assert!(usage, "`{lane}` has no USAGE on its own schema `{schema}`");
            } else {
                assert!(
                    !usage,
                    "CROSS-SERVICE LANE: `{lane}` holds USAGE on `{schema}`, a \
                     sibling's schema, and admits every tenant's rows there"
                );
                for table in tables_in(&pool, schema).await {
                    for verb in DML {
                        assert!(
                            !has_table_privilege(&pool, lane, &table, verb).await,
                            "CROSS-SERVICE LANE: `{lane}` can {verb} `{table}`"
                        );
                        checks += 1;
                    }
                }
            }
            checks += 1;
        }
        assert_eq!(
            has_table_privilege(&pool, lane, "audit.events", "INSERT").await,
            audits,
            "`{lane}`'s reach into the audit surface changed"
        );
        checks += 1;
    }

    assert_ran(checks);
}

/// Every verb the matrix rules on.
const DML: &[&str] = &["SELECT", "INSERT", "UPDATE", "DELETE"];

/// The tables a runtime role may only ADD to, and why each must never be rewritten.
const APPEND_ONLY: &[&str] = &["auth.feature_flags", "auth.organization_flags"];

/// The tables no runtime role may write AT ALL: the operator writes them as
/// the owner (`make flag`), and the answer to a key is its latest row, so an
/// INSERT moves a switch as surely as an UPDATE would.
const OPERATOR_ONLY: &[&str] = &["auth.feature_flags", "auth.organization_flags"];

/// The tables the runtime role only READS: its maintenance lane writes them,
/// and nothing a project-scoped request does may add to, rewrite or remove a
/// row. The delivery log is the record of what a project told the outside
/// world; its rows go with their delivery by cascade, which needs no grant.
/// An erasure's fence is read by every answer's save, and lifted or forged
/// by nothing a request does.
const LANE_WRITTEN: &[&str] = &["notifications.delivery_attempts", "agent.erasures"];

/// The runtime roles that write audit rows: every module's, since each
/// audits its own writes on the organization's chain.
const AUDIT_WRITERS: &[&str] = &["auth", "notifications"];

#[tokio::test]
async fn object_grants_give_each_role_its_own_schema_and_no_siblings() {
    let Some(pool) = harness().await else { return };
    let mut checks = 0usize;

    for &(role, own) in SERVICE_ROLES {
        assert!(
            has_schema_privilege(&pool, role, own, "USAGE").await,
            "role `{role}` has no USAGE on its own schema `{own}` — \
             the service cannot read a single row"
        );
        checks += 1;

        let children = partition_children(&pool, own).await;
        for table in tables_in(&pool, own).await {
            if children.contains(&table) {
                for verb in DML {
                    assert!(
                        !has_table_privilege(&pool, role, &table, verb).await,
                        "role `{role}` holds a DIRECT {verb} on partition child `{table}` — \
                         the grant belongs on the parent alone"
                    );
                    checks += 1;
                }
                continue;
            }
            assert!(
                has_table_privilege(&pool, role, &table, "SELECT").await,
                "role `{role}` cannot SELECT its own table `{table}` — \
                 healthy service, `permission denied` on every real request"
            );
            checks += 1;
            let operator_only = OPERATOR_ONLY.contains(&table.as_str());
            let lane_written = LANE_WRITTEN.contains(&table.as_str());
            assert_eq!(
                has_table_privilege(&pool, role, &table, "INSERT").await,
                !operator_only && !lane_written,
                "role `{role}` INSERT on `{table}`: an operator-only table is written \
                 by `make flag` as the owner and by no runtime role, a lane-written one \
                 by its maintenance lane alone, and every other table is one the \
                 service must be able to add to"
            );
            checks += 1;
            if lane_written {
                for verb in ["UPDATE", "DELETE"] {
                    assert!(
                        !has_table_privilege(&pool, role, &table, verb).await,
                        "role `{role}` can {verb} `{table}` — the lane writes it and a \
                         project only reads it; object_grants.sql revokes the rest"
                    );
                    checks += 1;
                }
            }
        }

        for &other in CROSS_SCHEMAS {
            if other == own {
                continue;
            }
            assert!(
                !has_schema_privilege(&pool, role, other, "USAGE").await,
                "role `{role}` has USAGE on sibling schema `{other}`"
            );
            checks += 1;
            for table in tables_in(&pool, other).await {
                for verb in DML {
                    assert!(
                        !has_table_privilege(&pool, role, &table, verb).await,
                        "CROSS-SERVICE VIOLATION: role `{role}` can {verb} `{table}`, \
                         which belongs to `{other}`"
                    );
                    checks += 1;
                }
            }
        }
    }

    assert_ran(checks);
}

/// Every runtime role can rewrite at least one of its own tables, so the
/// append-only refusals elsewhere in this file cannot pass vacuously on a
/// cluster that grants nobody UPDATE.
#[tokio::test]
async fn every_runtime_role_can_rewrite_a_mutable_table_of_its_own() {
    let Some(pool) = harness().await else { return };
    let mut checks = 0usize;

    for &(role, own) in SERVICE_ROLES {
        let children = partition_children(&pool, own).await;
        let Some(table) = tables_in(&pool, own).await.into_iter().find(|t| {
            !children.contains(t)
                && !APPEND_ONLY.contains(&t.as_str())
                && !LANE_WRITTEN.contains(&t.as_str())
        }) else {
            panic!(
                "schema `{own}` has no mutable table to probe — the UPDATE assertion \
                 for `{role}` just stopped running"
            );
        };
        assert!(
            has_table_privilege(&pool, role, &table, "UPDATE").await,
            "role `{role}` cannot UPDATE `{table}` — the append-only assertion \
             above would pass vacuously on a cluster that grants nobody UPDATE"
        );
        checks += 1;
    }

    assert_ran(checks);
}

/// The flag store, for every role that reaches it: read by both, written by
/// neither.
#[tokio::test]
async fn the_flag_store_is_written_by_the_operator_alone() {
    let Some(pool) = harness().await else { return };
    let mut checks = 0usize;

    for table in ["auth.feature_flags", "auth.organization_flags"] {
        for role in ["auth", "auth_maintenance"] {
            assert!(
                has_table_privilege(&pool, role, table, "SELECT").await,
                "role `{role}` cannot SELECT `{table}` — the resolver answers \
                 every flag ON, which is a kill switch that fails open"
            );
            checks += 1;
            for verb in ["INSERT", "UPDATE", "DELETE"] {
                assert!(
                    !has_table_privilege(&pool, role, table, verb).await,
                    "OPERATOR-ONLY VIOLATION: role `{role}` can {verb} `{table}` — \
                     the answer is the latest row per key, so an INSERT moves a \
                     switch as surely as a rewrite, and a rewrite erases the note \
                     saying who moved it; `make flag` writes as the owner"
                );
                checks += 1;
            }
        }
    }

    for verb in ["UPDATE", "DELETE"] {
        assert!(
            has_table_privilege(&pool, "auth", "auth.organizations", verb).await,
            "the auth role cannot {verb} `auth.organizations` — the append-only \
             refusals above would pass on any stripped database"
        );
        checks += 1;
    }

    assert_ran(checks);
}

/// The tenant root and the people, for the lane that spans every tenant. It
/// reads both — the deletion sweeps and `/me`'s organization list do — and
/// writes neither: every write to either runs under its own scope as the
/// service role. The auth migration grants the lane SELECT alone, and this
/// holds every tier to that answer.
#[tokio::test]
async fn the_maintenance_lane_reads_the_tenant_root_and_the_people_and_writes_neither() {
    let Some(pool) = harness().await else { return };
    let mut checks = 0usize;

    for table in ["auth.organizations", "auth.identities", "auth.accounts"] {
        assert!(
            has_table_privilege(&pool, "auth_maintenance", table, "SELECT").await,
            "auth_maintenance cannot SELECT `{table}` — the sweeps and `/me`'s \
             organization list read it in that lane"
        );
        checks += 1;
        for verb in ["INSERT", "UPDATE", "DELETE"] {
            assert!(
                !has_table_privilege(&pool, "auth_maintenance", table, verb).await,
                "auth_maintenance can {verb} `{table}` — a grant widened a lane \
                 that must only read it"
            );
            checks += 1;
        }
        assert!(
            has_table_privilege(&pool, "auth", table, "UPDATE").await,
            "the auth role cannot UPDATE `{table}` — the refusals above would pass \
             on any stripped database"
        );
        checks += 1;
    }

    assert_ran(checks);
}

/// ⚠ **No runtime role or lane touches a migration ledger.** The migrator keeps
/// `_sqlx_migrations` in each schema, and the own-schema grant's `ON ALL
/// TABLES` reaches it: a role holding DML there could mark a migration applied
/// that never ran, or erase one so the next deploy re-runs it.
#[tokio::test]
async fn no_runtime_role_can_touch_a_migration_ledger() {
    let Some(pool) = harness().await else { return };
    let mut checks = 0usize;

    let ledgers: Vec<String> = sqlx::query_scalar(
        "SELECT format('%I.%I', schemaname, tablename) FROM pg_tables
          WHERE tablename = '_sqlx_migrations' AND schemaname = ANY($1)",
    )
    .bind(CROSS_SCHEMAS)
    .fetch_all(&pool)
    .await
    .expect("list the ledgers");
    assert!(
        !ledgers.is_empty(),
        "no schema holds a migration ledger — the migrator's layout moved, and \
         this test stopped looking at anything"
    );
    let roles = SERVICE_ROLES
        .iter()
        .map(|(r, _)| *r)
        .chain(LANES.iter().map(|(l, _, _)| *l));
    for role in roles {
        for ledger in &ledgers {
            for verb in DML {
                assert!(
                    !has_table_privilege(&pool, role, ledger, verb).await,
                    "`{role}` can {verb} `{ledger}` — a runtime role can rewrite which \
                     migrations the next deploy believes have run"
                );
                checks += 1;
            }
        }
    }

    assert_ran(checks);
}

/// ⚠ **A new table is closed to every cross-tenant lane until its migration
/// grants it.** Each lane is granted table by table in its service's
/// migration; a DEFAULT PRIVILEGES entry naming a lane would open every future
/// table to every tenant through it.
#[tokio::test]
async fn no_lane_is_granted_a_future_table_by_default() {
    let Some(pool) = harness().await else { return };
    let mut checks = 0usize;

    for &(lane, _, _) in LANES {
        let defaults: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_default_acl d, aclexplode(d.defaclacl) a
              WHERE a.grantee = (SELECT oid FROM pg_roles WHERE rolname = $1)",
        )
        .bind(lane)
        .fetch_one(&pool)
        .await
        .expect("read the default privileges");
        assert_eq!(
            defaults, 0,
            "`{lane}` holds a DEFAULT PRIVILEGES grant — every table created from now \
             on opens to every tenant through it; grant it by name in the migration"
        );
        checks += 1;
    }

    assert_ran(checks);
}

/// Every other table the auth migration grants the lane less than full DML
/// on, held to the migration's answer. A privilege that differs between a
/// per-test database and a cloud tier once shipped a cross-tenant statement
/// its own tests could not reach.
#[tokio::test]
async fn the_maintenance_lanes_narrowed_grants_hold_in_every_tier() {
    let Some(pool) = harness().await else { return };
    let mut checks = 0usize;

    const NARROWED: &[(&str, &str, &[&str])] = &[
        ("auth_maintenance", "auth.confirmation_codes", &[]),
        // A password hash is read by its person alone; sign-in finds the
        // person by address through `auth.identities`.
        ("auth_maintenance", "auth.credentials", &[]),
        // The exchange lane resolves an external subject before any person
        // is known; the person it resolves to writes the link.
        ("auth_maintenance", "auth.external_identities", &["SELECT"]),
        // Every person request resolves its bearer before any person is
        // known, and a refresh mints the next under the lane; sign-out and
        // the sweep delete them. Nothing updates a bearer.
        (
            "auth_maintenance",
            "auth.access_tokens",
            &["SELECT", "INSERT", "DELETE"],
        ),
        // The exchange lane spends a code by deleting it, and the sweep
        // deletes the rest; the person writes them.
        (
            "auth_maintenance",
            "auth.authorization_codes",
            &["SELECT", "DELETE"],
        ),
        (
            "auth_maintenance",
            "auth.sessions",
            &["SELECT", "UPDATE", "DELETE"],
        ),
        (
            "auth_maintenance",
            "auth.member_invites",
            &["SELECT", "UPDATE", "DELETE"],
        ),
        (
            "auth_maintenance",
            "auth.organization_invites",
            &["SELECT", "UPDATE", "DELETE"],
        ),
        ("auth_maintenance", "auth.feature_flags", &["SELECT"]),
        ("auth_maintenance", "auth.organization_flags", &["SELECT"]),
        (
            "notifications_maintenance",
            "notifications.feed",
            &["SELECT", "INSERT", "UPDATE", "DELETE"],
        ),
        (
            "notifications_maintenance",
            "notifications.connections",
            &["SELECT", "UPDATE", "DELETE"],
        ),
        (
            "notifications_maintenance",
            "notifications.deliveries",
            &["SELECT", "INSERT", "UPDATE", "DELETE"],
        ),
        (
            "notifications_maintenance",
            "notifications.oauth_states",
            &["SELECT", "DELETE"],
        ),
        (
            "notifications_maintenance",
            "notifications.delivery_attempts",
            &["INSERT"],
        ),
        // The indexer writes every tenant's passages; retention and the
        // purges delete them.
        (
            "agent_maintenance",
            "agent.chunks",
            &["SELECT", "INSERT", "UPDATE", "DELETE"],
        ),
        // Retention and the purges delete a conversation; its person writes it.
        // (An erasure rewrites its `title` alone: a column grant, not this.)
        (
            "agent_maintenance",
            "agent.conversations",
            &["SELECT", "DELETE"],
        ),
        // Messages go with their conversation by cascade, which needs no grant.
        // (An erasure reads and rewrites `content` and `citations` alone:
        // column grants.)
        ("agent_maintenance", "agent.messages", &[]),
        ("agent_maintenance", "agent.cursors", &["SELECT", "UPDATE"]),
        // An erasure fences each organization it reaches, and retention
        // forgets the fence.
        (
            "agent_maintenance",
            "agent.erasures",
            &["SELECT", "INSERT", "UPDATE", "DELETE"],
        ),
    ];
    for (lane, table, allowed) in NARROWED {
        for verb in ["SELECT", "INSERT", "UPDATE", "DELETE"] {
            assert_eq!(
                has_table_privilege(&pool, lane, table, verb).await,
                allowed.contains(&verb),
                "{lane} {verb} on `{table}`: its service's migration grants {allowed:?}, \
                 and nothing else may widen it"
            );
            checks += 1;
        }
    }

    assert_ran(checks);
}

#[tokio::test]
async fn the_audit_surface_is_append_only_and_granted_on_the_parent_alone() {
    let Some(pool) = harness().await else { return };
    let mut checks = 0usize;

    for &(role, _) in SERVICE_ROLES {
        let writes_audit = AUDIT_WRITERS.contains(&role);
        assert_eq!(
            has_schema_privilege(&pool, role, "audit", "USAGE").await,
            writes_audit,
            "role `{role}` USAGE on `audit`: a writer without it cannot write its \
             audit row, and a failed audit is a failed operation; a role with no \
             audit call site holding it could only append rows it should not"
        );
        checks += 1;
        for verb in ["SELECT", "INSERT"] {
            assert_eq!(
                has_table_privilege(&pool, role, "audit.events", verb).await,
                writes_audit,
                "role `{role}` {verb} on audit.events is not what AUDIT_WRITERS says"
            );
            checks += 1;
        }
        for verb in ["UPDATE", "DELETE"] {
            assert!(
                !has_table_privilege(&pool, role, "audit.events", verb).await,
                "APPEND-ONLY VIOLATION: role `{role}` can {verb} audit.events"
            );
            checks += 1;
        }

        for child in audit_partitions(&pool).await {
            for verb in DML {
                assert!(
                    !has_table_privilege(&pool, role, &child, verb).await,
                    "PARTITION LEAK: role `{role}` can {verb} the audit child `{child}` \
                     directly, bypassing the parent's row-security policies"
                );
                checks += 1;
            }
        }
    }

    assert_ran(checks);
}

/// Connect, build the production role topology, and apply the real grants.
async fn harness() -> Option<PgPool> {
    let url = database_url_or_skip(module_path!())?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&url)
        .await
        .expect("connect to DATABASE_URL");

    SETUP.get_or_init(|| setup(pool.clone())).await;
    Some(pool)
}

/// Guards the one-time topology build (see [`harness`]).
static SETUP: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// Arbitrary constant key for the advisory lock that serializes setup.
const SETUP_LOCK: i64 = 0x7064_5f72_6f6c_6573; // "pd_roles" as bytes

/// Create the runtime roles and apply the grants, once at a time.
async fn setup(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire setup connection");

    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(SETUP_LOCK)
        .execute(&mut *conn)
        .await
        .expect("take the setup lock");

    let result = async {
        create_roles(&mut conn).await?;
        revoke_service_privileges(&mut conn).await?;
        conn.execute(OBJECT_GRANTS_SQL).await.map(|_| ())
    }
    .await;

    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(SETUP_LOCK)
        .execute(&mut *conn)
        .await;

    result.expect("build the role topology and apply object_grants.sql");
}

/// Create the runtime roles if they are absent.
async fn create_roles(conn: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    for role in REQUIRED_ROLES {
        let sql = format!(
            "DO $$ BEGIN \
               IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{role}') THEN \
                 CREATE ROLE {role} NOLOGIN; \
               END IF; \
             EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
        );
        conn.execute(sql.as_str()).await?;
    }
    Ok(())
}

/// Strip the runtime roles back to nothing before the grants are applied.
async fn revoke_service_privileges(conn: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    let roles = SERVICE_ROLES
        .iter()
        .map(|(r, _)| *r)
        .collect::<Vec<_>>()
        .join(", ");
    for schema in ["auth", "notifications", "agent", "audit"] {
        for stmt in [
            format!("REVOKE ALL ON ALL TABLES IN SCHEMA {schema} FROM {roles}"),
            format!("REVOKE ALL ON ALL SEQUENCES IN SCHEMA {schema} FROM {roles}"),
            format!("REVOKE ALL ON SCHEMA {schema} FROM {roles}"),
        ] {
            conn.execute(stmt.as_str()).await?;
        }
    }
    Ok(())
}

/// Guard against a run that asserts nothing and reports `ok`.
fn assert_ran(checks: usize) {
    assert!(
        checks > 0,
        "this test made zero assertions — the schemas it probes are absent. \
         Run `make db-reset` so the migrations are applied; a green run over an \
         empty database proves nothing."
    );
}

/// Whether `role` holds `privilege` on `table` **by a grant naming it directly**.
async fn has_table_privilege(pool: &PgPool, role: &str, table: &str, privilege: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
           SELECT 1 FROM pg_class c \
           JOIN pg_namespace n ON n.oid = c.relnamespace, \
           LATERAL aclexplode(c.relacl) acl \
           JOIN pg_roles g ON g.oid = acl.grantee \
           WHERE format('%I.%I', n.nspname, c.relname) = $2 \
             AND g.rolname = $1 AND acl.privilege_type = $3 )",
    )
    .bind(role)
    .bind(table)
    .bind(privilege)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("direct table privilege({role}, {table}, {privilege}): {e}"))
}

/// Schema-level counterpart of [`has_table_privilege`], read from `nspacl`.
async fn has_schema_privilege(pool: &PgPool, role: &str, schema: &str, privilege: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
           SELECT 1 FROM pg_namespace n, LATERAL aclexplode(n.nspacl) acl \
           JOIN pg_roles g ON g.oid = acl.grantee \
           WHERE n.nspname = $2 AND g.rolname = $1 AND acl.privilege_type = $3 )",
    )
    .bind(role)
    .bind(schema)
    .bind(privilege)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("direct schema privilege({role}, {schema}, {privilege}): {e}"))
}

/// Every table in `schema`, read from the catalogue so a new one is covered.
async fn tables_in(pool: &PgPool, schema: &str) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT format('%I.%I', schemaname, tablename) \
         FROM pg_tables WHERE schemaname = $1 AND tablename <> '_sqlx_migrations' \
         ORDER BY tablename",
    )
    .bind(schema)
    .fetch_all(pool)
    .await
    .unwrap_or_else(|e| panic!("list tables in {schema}: {e}"))
}

/// Every partition child in `schema`, whose grants live on its parent.
async fn partition_children(pool: &PgPool, schema: &str) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT format('%I.%I', n.nspname, c.relname) \
         FROM pg_inherits i \
         JOIN pg_class c ON c.oid = i.inhrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1",
    )
    .bind(schema)
    .fetch_all(pool)
    .await
    .unwrap_or_else(|e| panic!("list partition children in {schema}: {e}"))
}

/// The monthly child partitions of `audit.events`.
async fn audit_partitions(pool: &PgPool) -> Vec<String> {
    tables_in(pool, "audit")
        .await
        .into_iter()
        .filter(|t| t != "audit.events")
        .collect()
}

/// No table may use a legacy `serial` surrogate key.
#[tokio::test]
async fn no_table_uses_a_legacy_serial_surrogate_key() {
    let Some(pool) = harness().await else { return };

    let legacy: Vec<(String, String)> = sqlx::query_as(
        "SELECT (c.oid::regclass)::text, a.attname
           FROM pg_class c
           JOIN pg_namespace n ON n.oid = c.relnamespace
           JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0
          WHERE c.relkind = 'r'
            AND n.nspname IN ('auth','notifications','agent','audit')
            AND a.attidentity = ''
            AND pg_get_serial_sequence((c.oid::regclass)::text, a.attname) IS NOT NULL
          ORDER BY 1, 2",
    )
    .fetch_all(&pool)
    .await
    .expect("scan for serial columns");

    assert!(
        legacy.is_empty(),
        "legacy serial surrogate key(s) {legacy:?} — each needs a sequence \
         USAGE grant that object_grants.sql does not issue, so every INSERT \
         fails in production and passes here. Use \
         `BIGINT GENERATED ALWAYS AS IDENTITY`."
    );

    let tables_scanned: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c
           JOIN pg_namespace n ON n.oid = c.relnamespace
          WHERE c.relkind = 'r'
            AND n.nspname IN ('auth','notifications','agent','audit')",
    )
    .fetch_one(&pool)
    .await
    .expect("count scanned tables");
    assert!(
        tables_scanned >= 10,
        "only {tables_scanned} table(s) in the scanned schemas — the serial scan \
         above searched a near-empty catalogue and proved nothing. Run every \
         migration set (`make db-migrate`) and re-run."
    );
}
