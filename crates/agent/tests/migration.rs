//! The agent's one migration, held to the code with no database: each
//! `CHECK` on a column written through an enum admits exactly the enum's
//! words, and the cursors are the sources the indexer leases.
#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice,
    reason = "a panicking helper is a failing test"
)]

use std::collections::BTreeSet;
use std::fmt::Display;
use std::path::PathBuf;

use telmoni_agent::db::{MessageRole, Source, Visibility};
use telmoni_agent::index::Paged;

/// The crate's one migration.
fn migration() -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let found: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the migrations directory is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    let [path] = found.as_slice() else {
        panic!(
            "expected exactly one migration in {}, found {found:?} — one per crate before \
             v0.1.0",
            dir.display()
        );
    };
    std::fs::read_to_string(path).expect("the migration is readable")
}

/// The parenthesised body of a named `CHECK`.
fn check_body<'a>(sql: &'a str, constraint: &str) -> &'a str {
    let start = sql
        .find(constraint)
        .unwrap_or_else(|| panic!("`{constraint}` is not in the migration"));
    let rest = &sql[start..];
    let open = rest.find('(').expect("the constraint has a body");
    // Named in a comment before its definition, the parens read would be
    // another statement's.
    assert!(
        rest[..open].contains("CHECK"),
        "`{constraint}` is named but not defined there: {:?}",
        &rest[..open]
    );
    let mut depth = 0usize;
    for (offset, ch) in rest[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &rest[open..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("`{constraint}` has no balanced body");
}

/// Every `'quoted'` word in a stretch of SQL.
fn quoted(sql: &str) -> BTreeSet<String> {
    sql.split('\'')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// The words an enum's variants are written as.
fn words<T: Display>(all: impl IntoIterator<Item = T>) -> BTreeSet<String> {
    all.into_iter().map(|v| v.to_string()).collect()
}

/// ⚠ Each column is `TEXT` written through its enum: a word its `CHECK`
/// refuses fails every write of it, and one it admits that the enum lacks
/// is a row the code cannot read back, or no search finds.
#[test]
fn each_check_admits_exactly_its_enums_words() {
    let sql = migration();
    for (constraint, known) in [
        ("chunks_source_check", words(Source::all())),
        ("chunks_visibility_check", words(Visibility::all())),
        ("messages_role_check", words(MessageRole::all())),
    ] {
        let body = check_body(&sql, constraint);
        assert_eq!(quoted(body), known, "{constraint}: {body}");
    }
}

/// ⚠ A cursor row for each source the indexer leases, and only those: one
/// with no row is never leased, so never indexed, and nothing says so. The
/// docs lease theirs for a refresh; a conversation is remembered as it is
/// answered and leases none.
#[test]
fn the_cursors_are_the_sources_the_indexer_leases() {
    let sql = migration();
    let leased = words(
        Paged::ALL
            .map(Paged::source)
            .into_iter()
            .chain([Source::Docs]),
    );
    let body = check_body(&sql, "cursors_source_check");
    assert_eq!(quoted(body), leased, "cursors_source_check: {body}");
    let seeded = sql
        .split_once("INSERT INTO agent.cursors (source) VALUES")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map(|(rows, _)| quoted(rows))
        .expect("the cursors are seeded");
    assert_eq!(seeded, leased, "the cursors seeded");
}
