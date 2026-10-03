//! Every problem `type` a service can answer is documented, and nothing more.
//! The page lives in the sibling docs repository; without the repository the
//! test passes vacuously, and with it but without the page it fails.

#![expect(clippy::unwrap_used, reason = "test scaffolding")]

use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use telmoni_shared::test_util::{customer_docs_page, project_root};

/// Whole `type` URIs only: quoted in code, back-ticked on the page.
fn uris_in(text: &str, quote: char) -> BTreeSet<String> {
    let pattern = format!(r"{quote}(/errors/[a-z][a-z/-]*[a-z]){quote}");
    Regex::new(&pattern)
        .unwrap()
        .captures_iter(text)
        .map(|c| c[1].to_owned())
        .collect()
}

/// Every production `.rs` under `crates/*/src`, each cut off at its
/// `#[cfg(test)]` tail. A module may spell a problem type of its own, outside
/// `TelmoniError` — notifications does — and the page owes it a row too.
fn production_sources(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for crate_dir in std::fs::read_dir(root.join("crates")).unwrap() {
        let src = crate_dir.unwrap().path().join("src");
        if src.is_dir() {
            walk(&src, &mut files);
        }
    }
    files
        .into_iter()
        .map(|path| {
            let code = std::fs::read_to_string(path).unwrap();
            code.split("#[cfg(test)]").next().unwrap().to_owned()
        })
        .collect()
}

#[test]
fn the_error_page_and_the_error_code_name_the_same_types() {
    let root = project_root();
    let mut in_code = BTreeSet::new();
    for source in production_sources(&root) {
        in_code.extend(uris_in(&source, '"'));
    }
    let code = std::fs::read_to_string(root.join("crates/shared/src/error.rs")).unwrap();
    let production = code.split("#[cfg(test)]").next().unwrap();
    let slug_re = Regex::new(r#"\(\s*"([a-z-]+)",\s*(?:[0-9]{3},\s*)?"[^"]+""#).unwrap();
    for cap in slug_re.captures_iter(production) {
        let slug = &cap[1];
        let prefix = if ["forbidden", "insufficient-role"].contains(&slug) {
            "authz"
        } else {
            "auth"
        };
        in_code.insert(format!("/errors/{prefix}/{slug}"));
    }

    let Some(page) = customer_docs_page("errors.mdx") else {
        return;
    };
    let in_page = uris_in(&page, '`');

    let undocumented: Vec<_> = in_code.difference(&in_page).collect();
    let phantom: Vec<_> = in_page.difference(&in_code).collect();
    assert!(
        undocumented.is_empty(),
        "error types with no row in errors.mdx: {undocumented:?}"
    );
    assert!(
        phantom.is_empty(),
        "errors.mdx documents types no code produces: {phantom:?}"
    );
}
