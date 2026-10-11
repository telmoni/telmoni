//! `auth.api_tokens.scopes` must accept exactly the [`ApiKeyScope`] variants:
//! one the CHECK left out could never be minted, and a literal no variant
//! spells would be a scope nothing checks.
#![expect(clippy::string_slice, reason = "a panicking helper is a failing test")]
#![expect(clippy::expect_used, clippy::panic, reason = "test scaffolding")]

use std::fs;
use std::path::PathBuf;

use telmoni_shared::ApiKeyScope;

/// The crate's single pre-launch migration.
fn initial_migration() -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut found: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("auth migrations directory is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "sql")
        })
        .collect();
    found.sort();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one migration in {}",
        dir.display()
    );
    found
        .first()
        .map(|path| fs::read_to_string(path).expect("migration is readable"))
        .expect("the assertion above proves there is one")
}

/// The parenthesised body of a named CHECK constraint.
fn check_body(sql: &str, constraint: &str) -> String {
    let start = sql
        .find(&format!("CONSTRAINT {constraint}"))
        .unwrap_or_else(|| panic!("constraint `{constraint}` is absent from the migration"));
    let rest = &sql[start..];
    let open = rest.find('(').expect("a CHECK has a body");
    assert!(
        rest[..open].contains("CHECK"),
        "`{constraint}` is not a CHECK"
    );
    let mut depth = 0usize;
    for (offset, ch) in rest[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return rest[open..=open + offset].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("constraint `{constraint}` has no balanced paren body");
}

#[test]
fn the_check_names_every_scope_and_nothing_else() {
    let body = check_body(&initial_migration(), "api_tokens_scopes_check");
    let mut literals: Vec<&str> = body.split('\'').skip(1).step_by(2).collect();
    literals.sort_unstable();
    let mut scopes: Vec<String> = ApiKeyScope::all().iter().map(ToString::to_string).collect();
    scopes.sort();
    assert_eq!(
        literals, scopes,
        "the CHECK and ApiKeyScope disagree. CHECK body: {body}"
    );
    assert!(
        body.contains("cardinality(scopes) > 0"),
        "a key with no scope would open nothing and still be listed. CHECK body: {body}"
    );
}
