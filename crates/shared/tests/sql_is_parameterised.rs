//! Every value reaches SQL as a bind parameter — never spliced into the text.
//!
//! ⚠ **Row security guards against a handler's mistakes, not against SQL an
//! attacker writes.** Any role may run `set_config('app.organization_id', …)`
//! for itself, so one statement built from a request value could bind any
//! tenant it liked and every policy would agree. What holds the boundary is
//! that no request value is ever part of a statement's text. This test holds
//! the service source to it: a `format!` that builds SQL may splice in only a
//! `SCREAMING_CASE` constant (a column list, a shared `WHERE`), or one of the
//! identifiers listed below, each validated before it gets there. A positional
//! `{}` names nothing, so nothing can vouch for it, and it is never allowed.
#![expect(clippy::expect_used, reason = "test scaffolding")]

use std::path::{Path, PathBuf};

use regex::Regex;
use telmoni_shared::test_util::project_root;

/// Placeholders that are not constants, by file, and why each is safe. `*`
/// vouches for every placeholder in a file whose SQL is built from nothing a
/// request can reach.
const VALIDATED: &[(&str, &str, &str)] = &[
    (
        "crates/shared/src/db/retention.rs",
        "*",
        "partition DDL over `RETENTION`'s compile-time table definitions and a month's digits",
    ),
    (
        "crates/migrator/src/lib.rs",
        "schema",
        "a `Set`'s schema, which `Set::new` refuses unless `is_bare_identifier`",
    ),
    (
        "crates/shared/src/db.rs",
        "schema",
        "refused by `create_pool_for_service` unless a simple identifier",
    ),
    (
        "crates/shared/src/test_util/migrations.rs",
        "schema",
        "a test helper's own schema argument, never a request value",
    ),
    (
        "crates/shared/src/test_util/service_role.rs",
        "role",
        "`MIGRATOR_ROLE`, a `ServiceRole`'s name or lane, or a sibling's, asserted bare first",
    ),
    (
        "crates/shared/src/test_util/service_role.rs",
        "lane",
        "a `ServiceRole`'s lane, or a sibling's, which `sibling_pool` refuses unless bare",
    ),
    (
        "crates/auth/src/handler/export.rs",
        "sql",
        "every caller in the file passes a literal, or a format! of the file's own constants",
    ),
];

const SQL_START: &[&str] = &[
    "SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "SET", "CREATE", "ALTER", "GRANT", "REVOKE",
    "DROP", "DO",
];

/// A `format!` call's string literal. `(?s)` so that a `\` at the end of a
/// line, which continues the literal, does not end the match: a continued
/// literal is still one statement.
const FORMAT_CALL: &str = r#"(?s)format!\(\s*"((?:[^"\\]|\\.)*)""#;

/// The placeholders in one SQL `format!` literal from `rel` that splice in
/// anything but a constant or an identifier validated for that file, spelled
/// as written: `{id}`, or `{}` and `{0}`, which name nothing.
fn offenders_in(literal: &str, rel: &str) -> Vec<String> {
    let placeholder = Regex::new(r"\{([^{}:]*)(?::[^}]*)?\}").expect("regex");
    let constant = Regex::new(r"^[A-Z][A-Z0-9_]*$").expect("regex");
    let unescaped = literal.replace("{{", "").replace("}}", "");
    placeholder
        .captures_iter(&unescaped)
        .map(|c| c.get(1).map_or("", |m| m.as_str()))
        .filter(|name| {
            let positional = name.is_empty() || name.bytes().all(|b| b.is_ascii_digit());
            positional
                || !(constant.is_match(name)
                    || VALIDATED
                        .iter()
                        .any(|(f, n, _)| *f == rel && (*n == "*" || n == name)))
        })
        .map(|name| format!("{{{name}}}"))
        .collect()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir)
        .expect("a source dir is readable")
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn sql_text_splices_in_constants_and_validated_identifiers_only() {
    let root = project_root();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates/ is readable")
        .flatten()
    {
        let src = entry.path().join("src");
        if src.is_dir() {
            rust_sources(&src, &mut files);
        }
    }

    let format_call = Regex::new(FORMAT_CALL).expect("regex");

    let mut sql_formats = 0usize;
    let mut offenders = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .expect("under the root")
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(file).expect("a source file is readable");
        for call in format_call.captures_iter(&text) {
            let literal = call.get(1).map_or("", |m| m.as_str());
            let head = literal.trim_start().to_ascii_uppercase();
            if !SQL_START.iter().any(|k| head.starts_with(&format!("{k} "))) {
                continue;
            }
            sql_formats += 1;
            for placeholder in offenders_in(literal, &rel) {
                offenders.push(format!("{rel}: {placeholder} in \"{}\"", literal.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "SQL text built from a value that is not a constant — bind it (`$n` and \
         `.bind(…)`), or validate it and list it in VALIDATED with why:\n  {}",
        offenders.join("\n  ")
    );
    assert!(
        sql_formats > 10,
        "found only {sql_formats} SQL format! calls — the scan has drifted from how \
         queries are written, and a scan that matches nothing reports ok"
    );
    for (file, _, _) in VALIDATED {
        assert!(
            root.join(file).is_file(),
            "VALIDATED names {file}, which no longer exists — drop the entry"
        );
    }
}

#[test]
fn a_spliced_value_is_caught() {
    let format_call = Regex::new(FORMAT_CALL).expect("regex");
    // A constant, a named value, an escaped brace, and the two positional
    // forms — the ones the scan once let through, since they name nothing —
    // across a continued line, which once ended the match.
    let src = r#"sqlx::query(&format!(
        "SELECT {COLUMNS} FROM auth.x WHERE id = '{id}' AND tags = '{{}}' \
         AND a = {} AND b = {0:?}",
        a, b
    ))"#;
    let literal = format_call
        .captures(src)
        .and_then(|c| c.get(1))
        .expect("the fixture is a format! call")
        .as_str();
    assert_eq!(
        offenders_in(literal, "crates/auth/src/x.rs"),
        vec!["{id}", "{}", "{0}"]
    );
    // A validated identifier passes in its own file alone; `*` vouches for a
    // file's every name, and for no positional.
    assert!(offenders_in("SET search_path = {schema}", "crates/shared/src/db.rs").is_empty());
    assert_eq!(
        offenders_in("SET search_path = {schema}", "crates/auth/src/x.rs"),
        vec!["{schema}"]
    );
    assert_eq!(
        offenders_in(
            "DROP TABLE {schema}.{table}_{}",
            "crates/shared/src/db/retention.rs"
        ),
        vec!["{}"]
    );
}
