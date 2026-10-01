//! `auth.confirmation_codes.purpose` must accept exactly the [`Purpose`]
#![expect(clippy::string_slice, reason = "a panicking helper is a failing test")]
#![expect(clippy::expect_used, clippy::panic, reason = "test scaffolding")]

use std::fs;
use std::path::PathBuf;

use telmoni_auth::db::confirmation_codes::Purpose;

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
        "expected exactly one migration in {}, found {found:?} — one per crate \
         before v0.1.0, and a schema change edits it rather than adding a file",
        dir.display()
    );
    found
        .first()
        .map(|path| fs::read_to_string(path).expect("migration is readable"))
        .expect("the assertion above proves there is one")
}

/// Pull the parenthesised body of a named CHECK constraint.
fn check_body(sql: &str, constraint: &str) -> String {
    let start = sql
        .find(constraint)
        .unwrap_or_else(|| panic!("constraint `{constraint}` is absent from the migration"));
    let rest = &sql[start..];
    let open = rest.find('(').expect("a CHECK has a body");
    assert!(
        rest[..open].contains("CHECK"),
        "`{constraint}` is mentioned but not defined — the text between the name \
         and the next `(` is {:?}. This helper must not answer with some other \
         statement's parens.",
        &rest[..open]
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

/// Every `'quoted'` literal in a CHECK body.
fn literals(body: &str) -> Vec<&str> {
    body.split('\'').skip(1).step_by(2).collect()
}

#[test]
fn every_purpose_is_accepted() {
    let body = check_body(&initial_migration(), "confirmation_codes_purpose_check");
    for purpose in Purpose::all() {
        let literal = format!("'{}'", purpose.as_str());
        assert!(
            body.contains(&literal),
            "`{}` is a Purpose variant the CHECK rejects — nobody could ever \
             authorize that act. CHECK body: {body}",
            purpose.as_str()
        );
    }
}

#[test]
fn the_check_admits_no_purpose_the_enum_lacks() {
    let body = check_body(&initial_migration(), "confirmation_codes_purpose_check");
    let known: Vec<&str> = Purpose::all().iter().map(|p| p.as_str()).collect();
    for literal in literals(&body) {
        assert!(
            known.contains(&literal),
            "the CHECK admits `{literal}`, which is not a Purpose. The column is \
             TEXT and `find_live` binds `Purpose::as_str()`, so a code stored \
             under that spelling is minted, emailed, and never matches again."
        );
    }
}
