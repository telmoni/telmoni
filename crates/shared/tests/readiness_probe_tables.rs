//! Every readiness probe names a table its own service actually creates.
#![expect(clippy::expect_used, reason = "test scaffolding")]

use std::fs;
use std::path::{Path, PathBuf};

/// The readiness-probe shape: a statement that IS `SELECT 1 FROM
/// <schema>.<table> …`, so the literal opens with it. What follows the table
/// (a `LIMIT`, a keyed `WHERE`) is the probe's business; a `SELECT 1 FROM`
/// inside an `EXISTS` is some query's business and not a probe.
const PROBE_PREFIX: &str = "\"SELECT 1 FROM ";

/// Below this, the scanner has stopped finding probes rather than found them
/// clean: one per service that holds tables (auth, notifications, the agent
/// and telemetry).
const MIN_PROBES: usize = 4;

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root resolves from crates/shared")
        .to_path_buf()
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// `("notifications", "feed")` for each probe found in `src`.
fn probes() -> Vec<(String, String, PathBuf)> {
    let mut found = Vec::new();
    for crate_dir in fs::read_dir(project_root().join("crates"))
        .expect("crates/ is readable")
        .flatten()
    {
        let src = crate_dir.path().join("src");
        let mut files = Vec::new();
        rs_files(&src, &mut files);
        for f in files {
            let text = fs::read_to_string(&f).expect("source is readable");
            for line in text.lines() {
                let Some(rest) = line.split_once(PROBE_PREFIX).map(|(_, r)| r) else {
                    continue;
                };
                let Some(qualified) = rest.split_whitespace().next() else {
                    continue;
                };
                let Some((schema, table)) = qualified.trim_end_matches('"').split_once('.') else {
                    continue;
                };
                found.push((schema.to_owned(), table.to_owned(), f.clone()));
            }
        }
    }
    found
}

#[test]
fn every_readiness_probe_names_a_table_its_service_creates() {
    let probes = probes();
    assert!(
        probes.len() >= MIN_PROBES,
        "found {} readiness probe(s), expected at least {MIN_PROBES} — the scanner \
         is looking for the wrong shape, and a scanner that finds nothing reports \
         clean forever",
        probes.len(),
    );

    let mut ddl = String::new();
    for crate_dir in fs::read_dir(project_root().join("crates"))
        .expect("crates/ is readable")
        .flatten()
    {
        let Ok(migrations) = fs::read_dir(crate_dir.path().join("migrations")) else {
            continue;
        };
        for m in migrations.flatten() {
            ddl.push_str(&fs::read_to_string(m.path()).expect("migration is readable"));
        }
    }

    for (schema, table, file) in probes {
        let created = format!("CREATE TABLE {schema}.{table}");
        assert!(
            ddl.contains(&created),
            "{}: the readiness probe reads {schema}.{table}, and no migration \
             creates it. In a cluster this probe fails forever and the rollout \
             never completes.",
            file.display(),
        );
    }
}
