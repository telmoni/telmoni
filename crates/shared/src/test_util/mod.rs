//! In-memory test helpers + fixtures for telmoni-side service tests.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test scaffolding: asserts and fixture setup"
)]

pub mod id_token;
pub mod lanes;
pub mod migrations;
pub mod service_role;
pub mod tenancy;

pub use migrations::apply_audit_migrations;
pub use service_role::{ServiceRole, SiblingRole, service_pool, sibling_pool};
pub use tenancy::database_url_or_skip;

/// Record what a code exchange would have: `user`'s verified address, as the
/// identity provider asserted it. `/me` reads nothing else, and a session's
/// bearer names only somebody auth has recorded, so a suite that signs
/// somebody in calls this before minting one. Seeded as the owner, which
/// bypasses RLS.
///
/// The address is lowercased here as the exchange lowercases it, because the
/// column's CHECK refuses anything else.
pub async fn seed_identity(pool: &sqlx::PgPool, user: &str, email: &str) {
    sqlx::query(
        "INSERT INTO auth.identities (user_id, email, email_verified)
         VALUES ($1, $2, true)
         ON CONFLICT (user_id) DO UPDATE SET email = excluded.email",
    )
    .bind(user)
    .bind(email.trim().to_lowercase())
    .execute(pool)
    .await
    .expect("seed the identity an exchange records");
}

/// Whether some other session holds `organization`'s audit chain lock, the
/// key [`crate::audit::lock_chain`] takes, asked from a session of `pool`'s:
/// a try that fails is a lock held, and one that succeeds lets it go at once.
/// For a suite pinning that a change is decided, or a move made, under it.
pub async fn chain_lock_held(
    pool: &sqlx::PgPool,
    organization: &crate::types::OrganizationId,
) -> bool {
    let mut tx = pool.begin().await.expect("a probe transaction");
    let taken: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(organization)
            .fetch_one(&mut *tx)
            .await
            .expect("try the chain lock");
    tx.rollback().await.expect("let the probe's try go");
    !taken
}

/// Whether some other session holds `project`'s move lock alone, or waits
/// for it alone — the key [`crate::db::project_move::lock_for_move`] takes —
/// asked from a session of `pool`'s: a shared try that fails is a move under
/// way, and one that succeeds lets it go at once. For a suite pinning that a
/// move is made under it.
pub async fn move_lock_held(pool: &sqlx::PgPool, project: &crate::types::ProjectId) -> bool {
    let mut tx = pool.begin().await.expect("a probe transaction");
    let taken: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock_shared(hashtextextended('move:' || $1, 0))",
    )
    .bind(project)
    .fetch_one(&mut *tx)
    .await
    .expect("try the move lock");
    tx.rollback().await.expect("let the probe's try go");
    !taken
}

/// A session of `pool`'s holding `organization`'s audit chain lock until it is
/// rolled back, as a writer mid-change holds it.
pub async fn holding_chain(
    pool: &sqlx::PgPool,
    organization: &crate::types::OrganizationId,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut held = pool.begin().await.expect("a holding transaction");
    crate::audit::lock_chain(&mut held, organization)
        .await
        .expect("take the chain lock");
    held
}

/// Waits until at least `n` sessions of `pool`'s database are queued on an
/// advisory lock, failing after ten seconds.
pub async fn until_queued_on_advisory_locks(pool: &sqlx::PgPool, n: i64) {
    let mut queued = 0;
    for _ in 0..400 {
        queued = sqlx::query_scalar(
            "SELECT count(*) FROM pg_locks
              WHERE locktype = 'advisory' AND NOT granted
                AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
        )
        .fetch_one(pool)
        .await
        .expect("count the sessions queued on advisory locks");
        if queued >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        queued >= n,
        "{n} sessions never queued on an advisory lock within ten seconds"
    );
}

/// A pool whose connections can never be established: port 1 is reserved and
/// refuses. `connect_lazy` succeeds anyway, the production behaviour that once
/// let `/health` report Ready without ever reaching Postgres.
pub fn unreachable_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect_lazy("postgresql://nobody:nobody@127.0.0.1:1/nonexistent")
        .expect("a syntactically valid url builds a lazy pool")
}

/// Every log line written on this thread while the guard lives, at every
/// level, each with the fields of the spans it was written in, as JSON: for
/// a suite pinning that something reaches no log line, whatever on this
/// thread writes it. A `#[tokio::test]`'s runtime polls its tasks on the
/// thread that set it; a task sent to another thread, and a `log` record,
/// are not kept.
///
/// ⚠ Two subscribers that keep nothing are registered beside it for the
/// life of the process (`BYSTANDERS`). With one alone registered,
/// `tracing` asks whether a callsite is wanted of the thread that reaches it
/// first — in a suite run in parallel, another test's, which wants nothing —
/// and caches the answer, so a span this thread opens there later is never
/// printed: the request span, which every test of a router opens, went
/// missing from the lines four runs in ten. With more, it asks every
/// subscriber registered, under a lock — from the moment the second is,
/// which a callsite another thread reaches in that same instant may miss.
#[must_use]
pub fn captured_logs() -> (tracing::subscriber::DefaultGuard, CapturedLogs) {
    BYSTANDERS.get_or_init(|| {
        [
            tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()),
            tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()),
        ]
    });
    let logs = CapturedLogs::default();
    let sink = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || sink.clone())
        .finish();
    (tracing::subscriber::set_default(subscriber), logs)
}

/// What [`captured_logs`] registers beside each capture: two, so that one
/// capture beside them is never the only subscriber registered.
static BYSTANDERS: std::sync::OnceLock<[tracing::Dispatch; 2]> = std::sync::OnceLock::new();

/// The lines [`captured_logs`] keeps.
#[derive(Clone, Default)]
pub struct CapturedLogs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl CapturedLogs {
    /// Every line kept so far.
    #[must_use]
    pub fn text(&self) -> String {
        let kept = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(&kept).into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Resolves the repository root from `CARGO_MANIFEST_DIR`.
pub fn project_root() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root resolves from crates/shared")
        .to_path_buf()
}

/// Holds `value` to the JSON file at `path`: written there under
/// `UPDATE_CONTRACT` (`make contract`), else compared, so what changes the
/// file is a commit someone reads.
#[expect(clippy::panic, reason = "a missing or stale file is a failing test")]
pub fn pin_json(path: &std::path::Path, value: &serde_json::Value) {
    if std::env::var_os("UPDATE_CONTRACT").is_some() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("the file's directory");
        }
        let pretty = serde_json::to_string_pretty(value).expect("JSON serializes");
        std::fs::write(path, format!("{pretty}\n")).expect("the file is written");
        return;
    }
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{} ({e}): run `make contract`", path.display()));
    let on_disk: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} is not JSON ({e})", path.display()));
    assert_eq!(
        &on_disk,
        value,
        "{} is stale: run `make contract` and read the diff",
        path.display()
    );
}

/// [`pin_json`] for `contract/<name>` in this repository.
pub fn pin_contract(name: &str, value: &serde_json::Value) {
    pin_json(&project_root().join("contract").join(name), value);
}

/// A page of the customer documentation, by its path in the book,
/// `web/content/docs` (`"errors.mdx"`, `"api/field-inventory.mdx"`).
///
/// ⚠ **A page the book lacks panics.** That is a page that moved, and passing
/// over it once let a test check nothing for as long as its page lived
/// somewhere else.
#[must_use]
#[expect(
    clippy::panic,
    reason = "a book that lacks the page is a failing test, not a skipped one"
)]
pub fn customer_docs_page(page: &str) -> String {
    let book = project_root().join("web/content/docs");
    let path = book.join(page);
    if !path.is_file() {
        panic!(
            "no {page} under {}: the page moved, so point this test at its new path",
            book.display()
        );
    }
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} cannot be read: {e}", path.display()))
}

/// Recursively walks `dir` and calls `visit(path, content)` for each `.rs` file.
pub fn visit_rust_files(dir: &std::path::Path, visit: &mut dyn FnMut(&std::path::Path, &str)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "target" || name == ".git" || name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            visit_rust_files(&path, visit);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && let Ok(contents) = std::fs::read_to_string(&path)
        {
            visit(&path, &contents);
        }
    }
}

/// Every word of `T`'s table is the one serde writes and reads, and parses
/// and prints back to its variant: the derives held to [`crate::wire_enum!`].
pub fn assert_wire_words<T>()
where
    T: crate::types::WireEnum
        + serde::Serialize
        + serde::de::DeserializeOwned
        + std::str::FromStr
        + std::fmt::Display
        + std::fmt::Debug
        + PartialEq,
    <T as std::str::FromStr>::Err: std::fmt::Debug,
{
    for &variant in T::variants() {
        let word = variant.word();
        let quoted = format!("\"{word}\"");
        assert_eq!(
            serde_json::to_string(&variant).unwrap(),
            quoted,
            "{variant:?}"
        );
        assert_eq!(
            serde_json::from_str::<T>(&quoted).unwrap(),
            variant,
            "{word}"
        );
        assert_eq!(word.parse::<T>().unwrap(), variant, "{word}");
        assert_eq!(variant.to_string(), word, "{variant:?}");
    }
}
