//! The migration runner — `telmoni migrate` — and the nightly rotation —
//! `telmoni rotate`. Each runs once and returns, as the migrator's own
//! database role (`MIGRATOR_DATABASE_URL`) and ClickHouse user
//! (`MIGRATOR_CLICKHOUSE_URL`): a Kubernetes Job before the server rolls,
//! `make db-migrate` on a laptop, one command either way. Files are fully
//! qualified, so one connection runs every set without relying on
//! `search_path`.
//!
//! ClickHouse's half is telemetry's: its one file, applied after the
//! Postgres sets ([`telmoni_telemetry::schema`]), and its expired days and
//! months, dropped the same night as Postgres's partitions are rotated
//! ([`telmoni_telemetry::retention`]). Both read their URL before Postgres
//! is touched, so a missing one changes nothing.
//!
//! The sets are the directories under `MIGRATIONS_ROOT` — `/app/migrations`
//! unless set, one directory per schema, as the image lays them out — or the
//! `schema=dir` pairs `MIGRATION_SETS` lists, which is how a checkout names
//! its crates' `migrations/` directories (the Makefile owns that list). They
//! run in dependency order — `audit`, `auth`, then the rest by name — each
//! with its own ledger in its own schema, followed by the object grants and
//! every grants file: the ones `MIGRATION_GRANTS` lists, which is how a
//! checkout names a module's `grants.sql`, else every `.sql` under
//! `/app/grants`. An image built on the server's adds a set and its grants
//! by copying them in.

#![deny(missing_docs)]

mod rotate;

use std::path::{Path, PathBuf};

use anyhow::Context;

use telmoni_shared::config::{optional, require};

/// Run every pending migration of every set, then the object grants, then
/// ClickHouse's file.
pub async fn migrate_from_env() -> anyhow::Result<()> {
    let options = migrator_options()?;
    let clickhouse = telmoni_telemetry::store::client(&require(CLICKHOUSE_URL_VAR)?)
        .with_context(|| format!("{CLICKHOUSE_URL_VAR} is not a ClickHouse URL"))?;
    for set in sets_from_env()? {
        run_migrations(&options, &set).await?;
    }
    apply_object_grants(&options).await?;
    let statements = telmoni_telemetry::schema::apply(&clickhouse).await?;
    tracing::info!(statements, "clickhouse migration applied");
    Ok(())
}

/// Rotate every partitioned Postgres table once — create the months ahead,
/// and drop the expired ones where the registry allows it (nowhere today) —
/// then drop ClickHouse's days and months past retention, whether the first
/// failed or not. The first failure is the answer.
pub async fn rotate_from_env() -> anyhow::Result<()> {
    let options = migrator_options()?;
    let clickhouse = telmoni_telemetry::store::client(&require(CLICKHOUSE_URL_VAR)?)
        .with_context(|| format!("{CLICKHOUSE_URL_VAR} is not a ClickHouse URL"))?;
    // Both stores every night, whichever fails: a Postgres rotation that
    // fails must not hold back every night's purge behind it.
    let rotated = rotate::rotate(&options).await;
    let purged = telmoni_telemetry::retention::purge(&clickhouse, chrono::Utc::now()).await;
    if let Ok(purged) = &purged {
        tracing::info!(
            span_days = purged.span_days,
            rollup_months = purged.rollup_months,
            "rotate: clickhouse purged"
        );
    }
    rotated?;
    purged?;
    Ok(())
}

/// The variable naming the migrator's ClickHouse user, its user and password
/// carried as a DSN carries them. Read before anything runs; nothing is
/// dialled until the first statement.
const CLICKHOUSE_URL_VAR: &str = "MIGRATOR_CLICKHOUSE_URL";

/// The migrator's connection: `MIGRATOR_DATABASE_URL`, and the CA a pod
/// verifies the database with.
fn migrator_options() -> anyhow::Result<sqlx::postgres::PgConnectOptions> {
    let db_ca = telmoni_shared::db::database_ca_from_env().map_err(anyhow::Error::msg)?;
    let database_url = require("MIGRATOR_DATABASE_URL")?;
    Ok(telmoni_shared::db::connect_options(
        &database_url,
        db_ca.as_deref(),
    )?)
}

/// Where the image keeps one directory of migrations per schema, when
/// `MIGRATIONS_ROOT` does not say otherwise.
const DEFAULT_MIGRATIONS_ROOT: &str = "/app/migrations";

/// Where an image built on this one adds the grants its own sets need, one
/// `.sql` file per module, applied after [`OBJECT_GRANTS_SQL`] when
/// `MIGRATION_GRANTS` names no files itself.
const GRANTS_DIR: &str = "/app/grants";

/// The sets that must be present wherever the sets come from: the audit
/// table every handler writes, and the organizations and projects another
/// set's rows point at. A root or a list without them is an incomplete image
/// or a mistyped list, not a smaller deployment.
const CORE_SETS: [&str; 2] = ["audit", "auth"];

/// One schema's migrations: the schema its ledger and objects land in, and
/// the directory of `.sql` files.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Set {
    schema: String,
    dir: PathBuf,
}

impl Set {
    /// ⚠ The one constructor. `schema` is interpolated into `CREATE SCHEMA`
    /// and `SET search_path`, which cannot take a bind parameter, and it
    /// arrives from the environment or a directory name, so this check is
    /// the only thing between it and SQL.
    fn new(schema: &str, dir: PathBuf) -> anyhow::Result<Self> {
        if !is_bare_identifier(schema) {
            anyhow::bail!(
                "refusing to run migrations: {schema:?} (for {}) is not a bare lower-case SQL \
                 identifier",
                dir.display()
            );
        }
        Ok(Self {
            schema: schema.to_owned(),
            dir,
        })
    }
}

/// The sets to run, in order: `MIGRATION_SETS` when set, else every
/// directory under `MIGRATIONS_ROOT`.
fn sets_from_env() -> anyhow::Result<Vec<Set>> {
    let sets = match optional("MIGRATION_SETS") {
        Some(spec) => parse_sets(&spec)?,
        None => {
            let root =
                optional("MIGRATIONS_ROOT").unwrap_or_else(|| DEFAULT_MIGRATIONS_ROOT.to_owned());
            sets_under(Path::new(&root))?
        }
    };
    require_core(&sets)?;
    Ok(in_run_order(sets))
}

/// `schema=dir,schema=dir,…`, each pair checked before any set runs, so a
/// mistyped list fails with nothing applied.
fn parse_sets(spec: &str) -> anyhow::Result<Vec<Set>> {
    let mut sets: Vec<Set> = Vec::new();
    for pair in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let Some((schema, dir)) = pair.split_once('=') else {
            anyhow::bail!("MIGRATION_SETS entry {pair:?} is not `schema=dir`");
        };
        let (schema, dir) = (schema.trim(), dir.trim());
        if dir.is_empty() {
            anyhow::bail!("MIGRATION_SETS entry {pair:?} names no directory");
        }
        if sets.iter().any(|s| s.schema == schema) {
            anyhow::bail!("MIGRATION_SETS names {schema:?} twice");
        }
        sets.push(Set::new(schema, PathBuf::from(dir))?);
    }
    if sets.is_empty() {
        anyhow::bail!("MIGRATION_SETS is set but names no sets");
    }
    Ok(sets)
}

/// Every directory under `root`, named for its schema. Read from the
/// directory rather than listed, so an image built on this one adds a set by
/// copying it in.
fn sets_under(root: &Path) -> anyhow::Result<Vec<Set>> {
    let entries = std::fs::read_dir(root)
        .map_err(|e| anyhow::anyhow!("migrations root {} unreadable: {e}", root.display()))?;
    let mut sets = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            anyhow::bail!(
                "migrations root {} has an entry whose name is not UTF-8",
                root.display()
            );
        };
        sets.push(Set::new(&name, path)?);
    }
    if sets.is_empty() {
        anyhow::bail!("migrations root {} has no sets", root.display());
    }
    Ok(sets)
}

/// Refuse to run without every core set: a partial run would leave the
/// server booting against a database it cannot write.
fn require_core(sets: &[Set]) -> anyhow::Result<()> {
    for required in CORE_SETS {
        if !sets.iter().any(|s| s.schema == required) {
            let named: Vec<&str> = sets.iter().map(|s| s.schema.as_str()).collect();
            anyhow::bail!(
                "no `{required}` set among {named:?}; the image or MIGRATION_SETS is incomplete"
            );
        }
    }
    Ok(())
}

/// `audit`, then `auth`, then everything else by name.
fn in_run_order(mut sets: Vec<Set>) -> Vec<Set> {
    let rank = |s: &Set| {
        CORE_SETS
            .iter()
            .position(|core| *core == s.schema)
            .unwrap_or(CORE_SETS.len())
    };
    sets.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.schema.cmp(&b.schema)));
    sets
}

/// One set: its schema created if absent, `search_path` pinned to it so the
/// unqualified ledger and objects land there, then sqlx's runner.
async fn run_migrations(
    options: &sqlx::postgres::PgConnectOptions,
    set: &Set,
) -> anyhow::Result<()> {
    tracing::info!(
        migrations_dir = %set.dir.display(),
        schema = %set.schema,
        "running migrations"
    );

    let schema_for_hook = set.schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .after_connect(move |conn, _meta| {
            let schema = schema_for_hook.clone();
            Box::pin(async move {
                sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"))
                    .execute(&mut *conn)
                    .await?;
                sqlx::query(&format!("SET search_path TO {schema}, public"))
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options.clone())
        .await?;

    let migrator = sqlx::migrate::Migrator::new(set.dir.as_path()).await?;
    migrator.run(&pool).await?;

    tracing::info!(
        migrations_dir = %set.dir.display(),
        count = migrator.migrations.len(),
        "migrations complete"
    );
    Ok(())
}

/// The owner-issued object grants, embedded so they cannot drift from the
/// binary that applies them.
const OBJECT_GRANTS_SQL: &str = include_str!("../sql/object_grants.sql");

/// Every role the grants file names as a grantee, checked before executing.
/// A deployment that runs everything as one role has none of them, and
/// nothing to grant; a missing one is otherwise an error the file's own
/// guards cannot catch.
const GRANTEE_ROLES: &[&str] = &["auth", "notifications", "agent", "telemetry"];

/// Apply `object_grants.sql` as the owner, then every file in
/// [`GRANTS_DIR`], each skipped loudly — naming what is missing — when its
/// grantee roles don't exist.
async fn apply_object_grants(options: &sqlx::postgres::PgConnectOptions) -> anyhow::Result<()> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect_with(options.clone())
        .await?;

    let missing = missing_roles(&pool, GRANTEE_ROLES).await?;
    if !missing.is_empty() {
        tracing::warn!(
            missing = missing.join(", "),
            "skipping object grants: these roles do not exist, so no pool connects as them \
             (a single-role deployment); a tier and `make up` create them before migrating"
        );
        return Ok(());
    }
    sqlx::raw_sql(OBJECT_GRANTS_SQL).execute(&pool).await?;
    tracing::info!("object grants applied");

    for path in grants_files_from_env()? {
        let sql = std::fs::read_to_string(&path)?;
        let file = path.display().to_string();
        let Some(grantees) = declared_grantees(&sql) else {
            anyhow::bail!(
                "{file} names no grantees: its first line must be `-- grantees: <role>, …`"
            );
        };
        let missing = missing_roles(&pool, &grantees).await?;
        if !missing.is_empty() {
            tracing::warn!(
                file,
                missing = missing.join(", "),
                "skipping a grants file: grantee roles not present"
            );
            continue;
        }
        sqlx::raw_sql(&sql).execute(&pool).await?;
        tracing::info!(file, "grants file applied");
    }
    Ok(())
}

/// The grants files a module beyond this repository's adds, in the order
/// they run: the files `MIGRATION_GRANTS` lists, comma separated, else every
/// `.sql` under [`GRANTS_DIR`] by name. A listed file that is not there is
/// an error before anything is applied; an absent directory is no files.
fn grants_files_from_env() -> anyhow::Result<Vec<PathBuf>> {
    if let Some(spec) = optional("MIGRATION_GRANTS") {
        return parse_grants(&spec);
    }
    let mut files: Vec<PathBuf> = match std::fs::read_dir(GRANTS_DIR) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "sql"))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => anyhow::bail!("grants directory {GRANTS_DIR} unreadable: {e}"),
    };
    files.sort();
    Ok(files)
}

/// `file,file,…`, each checked to exist before any grant runs.
fn parse_grants(spec: &str) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let path = PathBuf::from(entry);
        if !path.is_file() {
            anyhow::bail!("MIGRATION_GRANTS names {entry:?}, which is not a file");
        }
        files.push(path);
    }
    if files.is_empty() {
        anyhow::bail!("MIGRATION_GRANTS is set but names no files");
    }
    Ok(files)
}

/// Which of `roles` the cluster lacks.
async fn missing_roles(pool: &sqlx::PgPool, roles: &[&str]) -> anyhow::Result<Vec<String>> {
    let existing: Vec<String> =
        sqlx::query_scalar("SELECT rolname FROM pg_roles WHERE rolname = ANY($1)")
            .bind(roles)
            .fetch_all(pool)
            .await?;
    Ok(roles
        .iter()
        .filter(|r| !existing.iter().any(|e| e == *r))
        .map(|r| (*r).to_owned())
        .collect())
}

/// The roles a grants file declares on its first line, `-- grantees: a, b`,
/// so it can be skipped before a missing one fails it halfway.
fn declared_grantees(sql: &str) -> Option<Vec<&str>> {
    let roles: Vec<&str> = sql
        .lines()
        .next()?
        .strip_prefix("-- grantees:")?
        .split(',')
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .collect();
    (!roles.is_empty()).then_some(roles)
}

/// Is this a bare, lower-case SQL identifier? ⚠ `CREATE SCHEMA` and
/// `SET search_path` cannot bind a parameter, and the value arrives from
/// `MIGRATION_SETS` or a directory name, so this check is the only thing
/// between it and SQL.
fn is_bare_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(schema: &str) -> Set {
        Set::new(schema, PathBuf::from(format!("crates/{schema}/migrations"))).unwrap()
    }

    fn schemas(sets: &[Set]) -> Vec<&str> {
        sets.iter().map(|s| s.schema.as_str()).collect()
    }

    /// ⚠ The guard on the two statements that cannot take a bind parameter.
    #[test]
    fn only_a_bare_lower_case_identifier_may_become_a_schema() {
        for good in ["audit", "auth", "notifications", "default", "a1_b"] {
            assert!(
                Set::new(good, PathBuf::from("x")).is_ok(),
                "{good:?} is a schema name the runner would now refuse"
            );
        }
        for bad in [
            "",
            "auth; DROP SCHEMA public CASCADE",
            "auth\"",
            "Auth",
            "1auth",
            "auth-service",
            "auth public",
            "pg_catalog\nauth",
            "aut\u{0068}\u{0301}",
        ] {
            assert!(
                Set::new(bad, PathBuf::from("x")).is_err(),
                "{bad:?} would be interpolated into CREATE SCHEMA"
            );
        }
        assert!(is_bare_identifier(&"a".repeat(63)));
        assert!(!is_bare_identifier(&"a".repeat(64)));
    }

    #[test]
    fn a_list_names_each_set_and_its_directory() {
        let sets = parse_sets(
            " audit=crates/migrator/migrations, auth = crates/auth/migrations ,\
             notifications=crates/notifications/migrations,",
        )
        .unwrap();
        assert_eq!(
            sets.iter()
                .map(|s| (s.schema.as_str(), s.dir.to_str().unwrap()))
                .collect::<Vec<_>>(),
            [
                ("audit", "crates/migrator/migrations"),
                ("auth", "crates/auth/migrations"),
                ("notifications", "crates/notifications/migrations"),
            ]
        );
    }

    /// Nothing runs off a list with a mistake in it.
    #[test]
    fn a_malformed_list_is_refused_before_anything_runs() {
        for bad in [
            "",
            " , ",
            "audit",
            "audit=",
            "=crates/migrator/migrations",
            "audit=a,audit=b",
            "Audit=crates/migrator/migrations",
        ] {
            assert!(parse_sets(bad).is_err(), "{bad:?} parsed");
        }
        assert!(require_core(&parse_sets("notifications=x").unwrap()).is_err());
        assert!(require_core(&parse_sets("audit=a,auth=b").unwrap()).is_ok());
    }

    /// The audit table before any handler's rows, auth's tables before any set
    /// that could point at them, and a set an image adds after both, wherever
    /// it sorts.
    #[test]
    fn audit_and_auth_run_first_and_an_added_set_after_them() {
        let sets = ["notifications", "zeta", "auth", "alpha", "audit"]
            .map(set)
            .to_vec();
        assert_eq!(
            schemas(&in_run_order(sets)),
            ["audit", "auth", "alpha", "notifications", "zeta"]
        );
    }

    /// A set this image lacks is not an error; one missing the core is.
    #[test]
    fn a_root_without_the_core_sets_is_refused() {
        let root = std::env::temp_dir().join(format!("migrator-root-{}", std::process::id()));
        std::fs::create_dir_all(root.join("audit")).unwrap();
        std::fs::create_dir_all(root.join("notifications")).unwrap();
        std::fs::write(root.join("README"), "not a set").unwrap();
        assert!(require_core(&sets_under(&root).unwrap()).is_err());

        std::fs::create_dir_all(root.join("auth")).unwrap();
        let sets = in_run_order(sets_under(&root).unwrap());
        assert!(require_core(&sets).is_ok());
        assert_eq!(schemas(&sets), ["audit", "auth", "notifications"]);
        assert_eq!(sets[0].dir, root.join("audit"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A checkout names a module's grants file; a name that is not a file
    /// fails before the owner's grants could be half applied.
    #[test]
    fn a_grants_list_names_files_that_exist_in_order() {
        let dir = std::env::temp_dir().join(format!("migrator-grants-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let b = dir.join("b.sql");
        let a = dir.join("a.sql");
        std::fs::write(&b, "-- grantees: svc\n").unwrap();
        std::fs::write(&a, "-- grantees: svc\n").unwrap();

        let listed = parse_grants(&format!(" {} , {} ,", b.display(), a.display())).unwrap();
        assert_eq!(
            listed,
            [b.clone(), a.clone()],
            "the list's order is the run order"
        );

        for bad in ["", " , ", dir.join("missing.sql").to_str().unwrap()] {
            assert!(parse_grants(bad).is_err(), "{bad:?} parsed");
        }
        assert!(
            parse_grants(dir.to_str().unwrap()).is_err(),
            "a directory is not a grants file"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A grants file must say whom it grants to on its first line, so a
    /// missing role skips the file rather than failing it halfway.
    #[test]
    fn a_grants_file_declares_its_grantees_on_its_first_line() {
        assert_eq!(
            super::declared_grantees("-- grantees: svc, svc_maintenance\nGRANT …;"),
            Some(vec!["svc", "svc_maintenance"])
        );
        assert_eq!(super::declared_grantees("GRANT …;\n-- grantees: svc"), None);
        assert_eq!(super::declared_grantees("-- grantees:\nGRANT …;"), None);
        assert_eq!(super::declared_grantees(""), None);
    }
}
