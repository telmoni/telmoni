//! The two migration files, held to the code and to the rules the tables are
//! built under, with no database: the content mode's `CHECK` accepts exactly
//! the modes Rust can hold, and the ClickHouse file runs again over itself
//! on one node anywhere, isolates every reader but the migrator, and lays
//! out `spans` as the module's row writes it.
#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    reason = "a panicking helper is a failing test"
)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use telmoni_shared::{ContentMode, ProjectId, SpanKind};
use telmoni_telemetry::schema::{MIGRATION, statements};
use telmoni_telemetry::store::{PROJECTS_SETTING, SpanRow};

/// The crate's one Postgres migration.
fn postgres_migration() -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let found: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the migrations directory is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one Postgres migration in {}, found {found:?} — one per crate \
         before v0.1.0",
        dir.display()
    );
    std::fs::read_to_string(&found[0]).expect("the migration is readable")
}

/// The parenthesised body of a named `CHECK`.
fn check_body<'a>(sql: &'a str, constraint: &str) -> &'a str {
    let start = sql
        .find(constraint)
        .expect("the constraint is in the migration");
    let rest = &sql[start..];
    let open = rest.find('(').expect("the constraint has a body");
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

/// ⚠ The column is `TEXT` read back through the enum: a mode the `CHECK`
/// refuses is a 500 at the first write of it, and one it admits that the
/// enum lacks fails every later read of the project's settings.
#[test]
fn the_content_mode_check_admits_exactly_the_modes_rust_holds() {
    let sql = postgres_migration();
    let body = check_body(&sql, "project_settings_content_mode_check");
    let admitted: BTreeSet<&str> = body.split('\'').skip(1).step_by(2).collect();
    let known: Vec<String> = ContentMode::all().iter().map(ToString::to_string).collect();
    let known: BTreeSet<&str> = known.iter().map(String::as_str).collect();
    assert_eq!(admitted, known, "CHECK body: {body}");
    assert!(
        sql.contains(&format!("DEFAULT '{}'", ContentMode::default())),
        "the column's default is not the enum's"
    );
}

/// ClickHouse's HTTP interface takes a statement at a time, and `migrate`
/// applies the whole file on every run: each statement must be one a second
/// run leaves alone.
#[test]
fn every_clickhouse_statement_is_if_not_exists() {
    let all = statements(MIGRATION);
    assert!(all.len() >= 7, "the file lost statements: {all:#?}");
    for statement in &all {
        let head: Vec<&str> = statement.split_whitespace().take(7).collect();
        assert!(
            head.first() == Some(&"CREATE")
                && head
                    .windows(3)
                    .any(|words| words == ["IF", "NOT", "EXISTS"]),
            "not a CREATE … IF NOT EXISTS: {}",
            head.join(" ")
        );
    }
    assert!(
        !MIGRATION.contains("/*"),
        "the splitter reads `--` comments alone"
    );
}

/// One node anywhere: no replication, no cluster, MergeTree engines in an
/// Atomic database, and no `CHECK` for a release to outrun.
#[test]
fn the_clickhouse_file_runs_on_one_node_anywhere() {
    let sql = statements(MIGRATION).join("\n");
    for refused in ["Replicated", "ON CLUSTER", "CONSTRAINT", "CHECK"] {
        assert!(!sql.contains(refused), "the ClickHouse file uses {refused}");
    }
    assert!(sql.contains("CREATE DATABASE IF NOT EXISTS telemetry ENGINE = Atomic"));
    assert!(sql.contains(
        "ENGINE = MergeTree\nPARTITION BY toDate(received_at)\nORDER BY (project_id, started_at)"
    ));
    assert!(sql.contains("ENGINE = AggregatingMergeTree\nPARTITION BY toYYYYMM(hour)"));
}

/// ⚠ Both tables answer only the projects a read's setting names, to every
/// user but the one the file runs as, who reads every row by a policy of its
/// own, whatever the server's default for a user no policy names; and the
/// view runs as that user, or the policy would fail every insert after its
/// spans were written.
#[test]
fn every_reader_but_the_migrator_is_held_and_the_view_runs_as_its_definer() {
    let all = statements(MIGRATION);
    for table in ["telemetry.spans", "telemetry.spans_hourly"] {
        let policy = all
            .iter()
            .find(|s| {
                s.starts_with(&format!(
                    "CREATE ROW POLICY IF NOT EXISTS tenant_isolation ON {table}\n"
                ))
            })
            .unwrap_or_else(|| panic!("{table} has no tenant_isolation policy"));
        assert!(
            policy.contains(&format!("getSetting('{PROJECTS_SETTING}')")),
            "{table}'s policy reads another setting: {policy}"
        );
        assert!(
            policy.ends_with("TO ALL EXCEPT CURRENT_USER"),
            "{table}'s policy holds the wrong users: {policy}"
        );
        let own = all
            .iter()
            .find(|s| {
                s.starts_with(&format!(
                    "CREATE ROW POLICY IF NOT EXISTS migrator_reads_all ON {table}\n"
                ))
            })
            .unwrap_or_else(|| panic!("{table} has no policy of the migrator's own"));
        // Pinned whole: permissive, as a policy is unless it says otherwise,
        // since a restrictive one would leave the migrator to the server's
        // default again; every row; and its own user alone.
        assert_eq!(
            *own,
            format!(
                "CREATE ROW POLICY IF NOT EXISTS migrator_reads_all ON {table}\n    \
                 USING 1\n    TO CURRENT_USER"
            ),
            "{table}'s migrator policy admits other rows or users, or restricts"
        );
    }
    // However laid out, and on whatever it names: a third permissive policy
    // would be OR-ed into what the module's user reads.
    let policies = all
        .iter()
        .filter(|s| s.to_ascii_uppercase().contains("ROW POLICY"))
        .count();
    assert_eq!(policies, 4, "a row policy beside each table's two");
    let view = all
        .iter()
        .find(|s| s.contains("MATERIALIZED VIEW"))
        .expect("the rollup's view");
    assert!(
        view.contains("DEFINER = CURRENT_USER SQL SECURITY DEFINER"),
        "{view}"
    );
    assert!(view.contains("TO telemetry.spans_hourly"), "{view}");
}

/// `spans` has a column for every field of the module's row and no other,
/// so an insert never meets a column it does not fill. Checked against a
/// server by the spans suite; here, without one.
#[test]
fn spans_has_a_column_for_every_field_of_the_row_and_no_other() {
    let create = statements(MIGRATION)
        .into_iter()
        .find(|s| s.starts_with("CREATE TABLE IF NOT EXISTS telemetry.spans\n"))
        .expect("the spans table");
    let open = create.find('(').expect("a column list");
    let close = create.find("\n)").expect("the column list's end");
    let columns: BTreeSet<String> = create[open + 1..close]
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect();

    let row = SpanRow {
        project_id: ProjectId::new(),
        received_at: chrono::Utc::now(),
        units: 1,
        run_id: uuid::Uuid::nil(),
        agent: String::new(),
        trace_id: String::new(),
        span_id: String::new(),
        parent_span_id: String::new(),
        kind: SpanKind::Run,
        name: String::new(),
        started_at: chrono::Utc::now(),
        ended_at: chrono::Utc::now(),
        status: telmoni_shared::SpanStatus::Ok,
        error_type: String::new(),
        model: String::new(),
        provider: String::new(),
        input_tokens: 0,
        output_tokens: 0,
        cache_read_input_tokens: 0,
        cache_write_input_tokens: 0,
        customer_id: String::new(),
        conversation_id: String::new(),
        tags: Vec::new(),
        bound_stall_after_s: None,
        bound_max_steps: None,
        bound_max_cost_usd: None,
        sample_rate: None,
        attributes: Vec::new(),
    };
    let fields: BTreeSet<String> = serde_json::to_value(&row)
        .expect("the row serializes")
        .as_object()
        .expect("the row is an object")
        .keys()
        .cloned()
        .collect();
    assert_eq!(columns, fields, "spans and SpanRow disagree on its columns");
}
