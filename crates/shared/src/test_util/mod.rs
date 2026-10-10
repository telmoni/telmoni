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
pub use service_role::{SiblingRole, service_pool, sibling_pool};
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

/// A pool whose connections can never be established: port 1 is reserved and
/// refuses. `connect_lazy` succeeds anyway, the production behaviour that once
/// let `/health` report Ready without ever reaching Postgres.
pub fn unreachable_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect_lazy("postgresql://nobody:nobody@127.0.0.1:1/nonexistent")
        .expect("a syntactically valid url builds a lazy pool")
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

/// A page of the customer documentation, by its path under the book
/// (`"errors.mdx"`, `"workspace/billing.mdx"`): the console's own
/// `web/content/docs`. Cargo names the package under test when it runs it, so
/// a repository built on this one reads its own pages first, the ones it lays
/// over the console, and this repository's for the rest.
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
    let core = project_root();
    // `project_root` is fixed when this crate is compiled, so it is this
    // repository even under another's tests; the package under test is only
    // known when the test runs.
    let built_on_it = std::env::var_os("CARGO_MANIFEST_DIR")
        .and_then(|manifest| {
            let manifest = std::path::PathBuf::from(manifest);
            Some(manifest.parent()?.parent()?.to_path_buf())
        })
        .filter(|root| *root != core);
    let homes: Vec<std::path::PathBuf> = built_on_it
        .into_iter()
        .chain(std::iter::once(core))
        .map(|root| root.join("web/content/docs"))
        .collect();
    let Some(path) = homes
        .iter()
        .map(|home| home.join(page))
        .find(|path| path.is_file())
    else {
        panic!(
            "no {page} under {}: the page moved, so point this test at its new path",
            homes
                .iter()
                .map(|home| home.display().to_string())
                .collect::<Vec<_>>()
                .join(" or ")
        );
    };
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
