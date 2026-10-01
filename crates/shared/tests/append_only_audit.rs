//! Append-only invariant for the `audit.events` table.

use telmoni_shared::test_util::{project_root, visit_rust_files};

/// The only paths permitted to issue UPDATE/DELETE on `audit.events`.
const ALLOWED: &[&str] = &["crates/shared/src/audit.rs"];

#[test]
fn no_unsanctioned_writes_to_audit_events() {
    let root = project_root();
    let mut violations = Vec::new();

    visit_rust_files(&root.join("crates"), &mut |path, contents| {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        if ALLOWED.iter().any(|allowed| rel == *allowed) {
            return;
        }

        if rel.ends_with("tests/append_only_audit.rs") {
            return;
        }

        for (lineno, line) in contents.lines().enumerate() {
            let lower = line.to_ascii_lowercase();
            if lower.contains("update audit.events")
                || lower.contains("delete from audit.events")
                || lower.contains("insert into audit.events")
            {
                violations.push(format!("{rel}:{} → {}", lineno + 1, line.trim()));
            }
        }
    });

    assert!(
        violations.is_empty(),
        "audit.events is append-only and single-writer — only `{}` may INSERT/UPDATE/DELETE it; \
         every other path must go through `telmoni_shared::audit::emit_audit` (the advisory-locked \
         writer that keeps each organization's `seq` chain linear). Violations:\n  {}",
        ALLOWED.join("`, `"),
        violations.join("\n  "),
    );
}
