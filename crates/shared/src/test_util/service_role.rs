//! A pool that connects as a **production service role**, so RLS applies.
//!
//! ⚠ `#[sqlx::test]` connects as the database owner, a SUPERUSER, and
//! `BYPASSRLS` overrides exactly the `FORCE` every tenant table carries — so no
//! handler test could observe an RLS-blocked read, which is how
//! `acting_project` shipped a lookup bound to the wrong GUC under a green
//! suite. [`service_pool`] applies the REAL `object_grants.sql` and `SET ROLE`s
//! to the role the deployed process runs as.
//!
//! **Give it to the handler, not the test.** Fixtures seed and assertions read
//! with the owner's pool; only `AppState.db` gets this one, so what is under
//! test is the handler's own reach.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor as _, PgPool};
use tokio::sync::Mutex;

pub use crate::db::tenant_session::ServiceRole;

/// The owner-issued object grants, byte-identical to what the migrator binary
/// embeds. Read at compile time, so a moved file is a build error rather than
/// a suite that silently grants nothing.
const OBJECT_GRANTS_SQL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../migrator/sql/object_grants.sql"
));

/// The role `object_grants.sql`'s `ALTER DEFAULT PRIVILEGES` names, beside
/// the modules' roles and lanes ([`ServiceRole`]): it fails without it.
const MIGRATOR_ROLE: &str = "migrator";

/// A service beside this repository's that runs under the same rules: its
/// login role, the lane role only it may enter, and the owner-issued grants
/// its migration set adds (the migrator applies them from its grants
/// directory, after `object_grants.sql`).
#[derive(Debug, Clone, Copy)]
pub struct SiblingRole {
    /// The login role the service's process runs as.
    pub role: &'static str,
    /// Its NOLOGIN maintenance lane, when it has one.
    pub lane: Option<&'static str>,
    /// Its grants file, applied after this repository's.
    pub grants_sql: &'static str,
}

/// Serialises the cluster-wide half of setup (see [`prepare`]).
const SETUP_LOCK: i64 = 0x7376_635f_726f_6c65; // "svc_role" as bytes

/// Databases whose grants are already in place, so the DDL runs once per test
/// database rather than per connection. Keyed by database because object
/// grants are per-database.
static PREPARED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// A pool connected as `role`, with the production grants applied to the
/// calling test's database.
#[must_use = "the pool is what the handler must be given; dropping it changes nothing"]
pub fn service_pool(pool: &PgPool, role: ServiceRole) -> PgPool {
    role_pool(pool, role.name(), None)
}

/// [`service_pool`] for a service beside this repository's: this
/// repository's topology and grants first, then the sibling's role, lane and
/// grants, once per database. Panics unless the role and the lane are bare
/// identifiers, since both are spliced into the statements that create them.
#[must_use = "the pool is what the handler must be given; dropping it changes nothing"]
pub fn sibling_pool(pool: &PgPool, sibling: SiblingRole) -> PgPool {
    for name in std::iter::once(sibling.role).chain(sibling.lane) {
        assert!(
            is_bare_identifier(name),
            "`{name}` is not a bare role name (lowercase letters, digits and `_`)"
        );
    }
    role_pool(pool, sibling.role, Some(sibling))
}

/// A lowercase SQL identifier that needs no quoting.
fn is_bare_identifier(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn role_pool(pool: &PgPool, role: &str, sibling: Option<SiblingRole>) -> PgPool {
    let connect_options = Arc::unwrap_or_clone(pool.connect_options());
    let database = connect_options
        .get_database()
        .expect("the test pool names a database")
        .to_owned();

    let set_role = format!("SET ROLE {role}");

    PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(30))
        .after_connect(move |conn, _meta| {
            let database = database.clone();
            let set_role = set_role.clone();
            Box::pin(async move {
                prepare(conn, &database, sibling).await;
                conn.execute(set_role.as_str())
                    .await
                    .expect("SET ROLE to the service role");
                Ok(())
            })
        })
        .after_release(|_conn, _meta| Box::pin(async { Ok(false) }))
        .connect_lazy_with(connect_options)
}

/// Build the production role topology and apply the real grants, once per
/// database, and a sibling's on top once per database and sibling. Roles are
/// cluster-wide, so parallel test binaries race on them — Postgres reports
/// that race as `unique_violation`, not `duplicate_object` — which is why the
/// lock is here.
async fn prepare(conn: &mut sqlx::PgConnection, database: &str, sibling: Option<SiblingRole>) {
    let sibling_key = sibling.map(|s| format!("{database}/{}", s.role));
    let mut prepared = PREPARED.lock().await;
    let core_done = prepared.contains(database);
    let sibling_done = sibling_key
        .as_ref()
        .is_none_or(|key| prepared.contains(key));
    if core_done && sibling_done {
        return;
    }

    conn.execute("BEGIN")
        .await
        .expect("begin service-role setup");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SETUP_LOCK)
        .execute(&mut *conn)
        .await
        .expect("take the service-role setup lock");

    if !core_done {
        create_role(conn, MIGRATOR_ROLE).await;
        for role in ServiceRole::all() {
            let (login, lane) = (role.name(), role.lane().role());
            create_role(conn, login).await;
            create_role(conn, lane).await;
            // As the role hardening grants it: its own lane only, and never
            // inherited.
            let membership = format!("GRANT {lane} TO {login} WITH INHERIT FALSE, SET TRUE");
            conn.execute(membership.as_str())
                .await
                .expect("grant a maintenance lane membership");
        }
        conn.execute(OBJECT_GRANTS_SQL)
            .await
            .expect("apply object_grants.sql");
    }

    if let (Some(sibling), false) = (sibling, sibling_done) {
        create_role(conn, sibling.role).await;
        if let Some(lane) = sibling.lane {
            create_role(conn, lane).await;
            let membership = format!(
                "GRANT {lane} TO {role} WITH INHERIT FALSE, SET TRUE",
                role = sibling.role
            );
            conn.execute(membership.as_str())
                .await
                .expect("grant the sibling's lane membership");
        }
        conn.execute(sibling.grants_sql)
            .await
            .expect("apply the sibling's grants");
    }

    conn.execute("COMMIT")
        .await
        .expect("commit service-role setup");

    prepared.insert(database.to_owned());
    if let Some(key) = sibling_key {
        prepared.insert(key);
    }
}

/// `CREATE ROLE <role> NOLOGIN`, tolerating one a parallel binary made first.
async fn create_role(conn: &mut sqlx::PgConnection, role: &str) {
    let sql = format!(
        "DO $$ BEGIN \
           IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{role}') THEN \
             CREATE ROLE {role} NOLOGIN; \
           END IF; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
    );
    conn.execute(sql.as_str())
        .await
        .expect("create a service role");
}
