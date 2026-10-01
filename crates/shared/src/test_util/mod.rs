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

/// A page of the customer documentation site, by its path under
/// `src/content/docs/` (`"errors.mdx"`), read from a checkout inside this
/// repository or beside it (`../docs`).
///
/// `None` only when no docs site is here at all, so a test holding the code
/// to a page passes vacuously where the site is not checked out, as in CI.
/// ⚠ **A site that is here but lacks the page panics.** That is a page that
/// moved, and answering `None` for it once let a test check nothing for as
/// long as its page lived somewhere else.
#[must_use]
#[expect(
    clippy::panic,
    reason = "a docs site that lacks the page is a failing test, not a skipped one"
)]
pub fn customer_docs_page(page: &str) -> Option<String> {
    let root = project_root();
    let site = std::iter::once(root.join("docs/src/content/docs"))
        .chain(
            root.parent()
                .map(|parent| parent.join("docs/src/content/docs")),
        )
        .find(|dir| dir.is_dir())?;
    let path = site.join(page);
    Some(std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "the docs site at {} has no {page} ({e}): the page moved, so point this test at \
             its new path",
            site.display()
        )
    }))
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
