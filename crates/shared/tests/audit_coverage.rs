//! Audit-completeness CI gate: a manifest of every mutating handler, asserted
//! in both directions, so an unaudited route is a build failure.
#![expect(
    clippy::indexing_slicing,
    clippy::string_slice,
    reason = "a panicking helper is a failing test"
)]
#![expect(
    clippy::expect_used,
    reason = "test scaffolding: asserts and fixture setup"
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
enum ActorPattern {
    /// Service actor — `Actor::Service("<name>")`, rendering `service:<name>`.
    Service(&'static str),
    /// External actor — `Actor::External("<name>")`, rendering `external:<name>`
    External(&'static str),
    /// User actor — `Actor::User(<expr>)` from a session, header or token.
    Variable,
    /// The lane's principal, passed in as a binding named `actor`.
    #[expect(dead_code, reason = "manifest vocabulary awaiting its next handler")]
    Principal,
}

#[derive(Debug, Clone, Copy)]
struct Mutation {
    file: &'static str,
    fn_name: &'static str,
    /// `AuditAction` variant name, matched with any prefix.
    action: &'static str,
    /// `TelmoniResourceKind` variant name.
    resource_kind: &'static str,
    actor_id: ActorPattern,
}

const MUTATIONS: &[Mutation] = &[
    Mutation {
        file: "crates/auth/src/handler/tokens.rs",
        fn_name: "create_token",
        action: "Created",
        resource_kind: "Token",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/tokens.rs",
        fn_name: "rotate_token",
        action: "Updated",
        resource_kind: "Token",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/tokens.rs",
        fn_name: "revoke_token",
        action: "Deleted",
        resource_kind: "Token",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/notifications/src/handler/connectors.rs",
        fn_name: "callback",
        action: "Created",
        resource_kind: "Connector",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/notifications/src/handler/connectors.rs",
        fn_name: "create_webhook",
        action: "Created",
        resource_kind: "Connector",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/notifications/src/handler/connectors.rs",
        fn_name: "rotate_webhook_secret",
        action: "Updated",
        resource_kind: "Connector",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/notifications/src/handler/connectors.rs",
        fn_name: "update_webhook_events",
        action: "Updated",
        resource_kind: "Connector",
        actor_id: ActorPattern::Variable,
    },
    // `redeliver`'s audit row, written once the delivery's outcome is known.
    Mutation {
        file: "crates/notifications/src/handler/connectors.rs",
        fn_name: "record_redelivery_outcome",
        action: "Updated",
        resource_kind: "Connector",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/notifications/src/handler/connectors.rs",
        fn_name: "delete_connector",
        action: "Deleted",
        resource_kind: "Connector",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/notifications/src/handler/slack_events.rs",
        fn_name: "revoke_workspace",
        action: "Updated",
        resource_kind: "Connector",
        actor_id: ActorPattern::External("slack"),
    },
    Mutation {
        file: "crates/auth/src/handler/organization.rs",
        fn_name: "delete_organization",
        action: "Updated",
        resource_kind: "Organization",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/organization.rs",
        fn_name: "restore_organization",
        action: "Updated",
        resource_kind: "Organization",
        actor_id: ActorPattern::Variable,
    },
    // The operator's two, run as `telmoni terminate` and `telmoni restore`.
    Mutation {
        file: "crates/auth/src/sweep.rs",
        fn_name: "terminate",
        action: "Updated",
        resource_kind: "Organization",
        actor_id: ActorPattern::Service("operator"),
    },
    Mutation {
        file: "crates/auth/src/sweep.rs",
        fn_name: "restore",
        action: "Updated",
        resource_kind: "Organization",
        actor_id: ActorPattern::Service("operator"),
    },
    Mutation {
        file: "crates/auth/src/handler/account.rs",
        fn_name: "delete_account",
        action: "Updated",
        resource_kind: "Organization",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/account.rs",
        fn_name: "confirm_email_change",
        action: "Updated",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/organization.rs",
        fn_name: "rename_organization",
        action: "Updated",
        resource_kind: "Organization",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/ownership.rs",
        fn_name: "offer",
        action: "Updated",
        resource_kind: "Organization",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/ownership.rs",
        fn_name: "cancel",
        action: "Updated",
        resource_kind: "Organization",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/ownership.rs",
        fn_name: "decline",
        action: "Updated",
        resource_kind: "Organization",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/ownership.rs",
        fn_name: "accept",
        action: "Updated",
        resource_kind: "Organization",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/project_transfer.rs",
        fn_name: "offer",
        action: "Updated",
        resource_kind: "Project",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/project_transfer.rs",
        fn_name: "cancel",
        action: "Updated",
        resource_kind: "Project",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/project_transfer.rs",
        fn_name: "decline",
        action: "Updated",
        resource_kind: "Project",
        actor_id: ActorPattern::Variable,
    },
    // `accept`'s audit rows, on both organizations' chains.
    Mutation {
        file: "crates/auth/src/handler/project_transfer.rs",
        fn_name: "emit_project_transfer_audits",
        action: "Updated",
        resource_kind: "Project",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/project_transfer.rs",
        fn_name: "emit_project_transfer_audits",
        action: "Deleted",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/project_transfer.rs",
        fn_name: "emit_project_transfer_audits",
        action: "Created",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/export.rs",
        fn_name: "export_organization",
        action: "Exported",
        resource_kind: "Organization",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/account.rs",
        fn_name: "set_analytics_preference",
        action: "Updated",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/me.rs",
        fn_name: "provision_first_organization",
        action: "Created",
        resource_kind: "Organization",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/me.rs",
        fn_name: "provision_first_organization",
        action: "Created",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/me.rs",
        fn_name: "provision_first_organization",
        action: "Created",
        resource_kind: "Project",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/deletion.rs",
        fn_name: "finalize_organization",
        action: "Deleted",
        resource_kind: "Organization",
        actor_id: ActorPattern::Service("auth"),
    },
    Mutation {
        file: "crates/auth/src/handler/invite.rs",
        fn_name: "create_invite",
        action: "Created",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/invite.rs",
        fn_name: "revoke_invite",
        action: "Deleted",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/invite.rs",
        fn_name: "seat_project_member",
        action: "Created",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/invite.rs",
        fn_name: "seat_organization_member",
        action: "Created",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/invite.rs",
        fn_name: "decline_my_incoming_invite",
        action: "Deleted",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/member.rs",
        fn_name: "update_role",
        action: "Updated",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/member.rs",
        fn_name: "remove_member",
        action: "Deleted",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/member.rs",
        fn_name: "leave",
        action: "Deleted",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/projects.rs",
        fn_name: "create_project",
        action: "Created",
        resource_kind: "Project",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/projects.rs",
        fn_name: "delete_project",
        action: "Deleted",
        resource_kind: "Project",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/organization_members.rs",
        fn_name: "update_organization_member_role",
        action: "Updated",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/organization_members.rs",
        fn_name: "remove_organization_member",
        action: "Deleted",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/organization_members.rs",
        fn_name: "create_organization_invite",
        action: "Created",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/organization_members.rs",
        fn_name: "revoke_organization_invite",
        action: "Deleted",
        resource_kind: "Member",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/sessions.rs",
        fn_name: "revoke",
        action: "Deleted",
        resource_kind: "Session",
        actor_id: ActorPattern::Variable,
    },
    Mutation {
        file: "crates/auth/src/handler/deletion.rs",
        fn_name: "erase_person",
        action: "Deleted",
        resource_kind: "Member",
        actor_id: ActorPattern::Service("deletion-saga"),
    },
];

#[test]
fn every_listed_mutation_emits_expected_tuple() {
    let root = project_root();
    let mut violations = Vec::new();

    for m in MUTATIONS {
        let abs = root.join(m.file);
        let Ok(contents) = fs::read_to_string(&abs) else {
            violations.push(format!(
                "{}::{} → file not found at {}",
                m.file,
                m.fn_name,
                abs.display(),
            ));
            continue;
        };

        let Some(body) = slurp_fn_body(&contents, m.fn_name) else {
            violations.push(format!(
                "{}::{} → `(pub) async fn {}` not found",
                m.file, m.fn_name, m.fn_name,
            ));
            continue;
        };

        let emits = parse_emit_audit_calls(&body);
        if emits.is_empty() {
            violations.push(format!(
                "{}::{} → no `emit_audit(...)` calls in fn body",
                m.file, m.fn_name,
            ));
            continue;
        }

        let matched = emits.iter().any(|e| {
            normalize_action(&e.action) == m.action
                && normalize_resource_kind(&e.resource_kind) == m.resource_kind
                && actor_matches(&e.actor_id, m.actor_id)
        });

        if !matched {
            let want = format!(
                "(action: {}, resource_kind: {}, actor_id: {})",
                m.action,
                m.resource_kind,
                describe_pattern(m.actor_id),
            );
            let found = emits
                .iter()
                .map(|e| {
                    format!(
                        "(action: {}, resource_kind: {}, actor_id: {})",
                        normalize_action(&e.action),
                        normalize_resource_kind(&e.resource_kind),
                        e.actor_id,
                    )
                })
                .collect::<Vec<_>>()
                .join(" / ");
            violations.push(format!(
                "{}::{} → expected {}, found {}",
                m.file, m.fn_name, want, found,
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "MUTATIONS manifest drifted from handler source — \
         fix the row or the call, whichever's wrong:\n  {}",
        violations.join("\n  "),
    );
}

/// Crates whose `src/handler` tree this gate scans.
const SCANNED_SERVICES: &[&str] = &["auth", "notifications"];

/// Every crate that HAS a `src/handler` tree must be in [`SCANNED_SERVICES`].
#[test]
fn services_with_handlers_are_all_scanned() {
    let root = project_root();
    let mut unscanned = Vec::new();
    let crates_dir = root.join("crates");
    let entries = std::fs::read_dir(&crates_dir).expect("read crates/");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.path().join("src/handler").is_dir() {
            continue;
        }
        if !SCANNED_SERVICES.contains(&name.as_str()) {
            unscanned.push(name);
        }
    }
    unscanned.sort();
    assert!(
        unscanned.is_empty(),
        "crate(s) have a src/handler tree but are absent from SCANNED_SERVICES: {unscanned:?}.\n  \
         Their audited mutations are not failing — they are unexamined. Add them, \
         then add a MUTATIONS row per emitting handler."
    );
}

/// ⚠ **THE HOLE THE OTHER TWO TESTS LEAVE.**
#[test]
fn every_project_creation_is_audited() {
    let root = project_root();
    let mut silent = Vec::new();

    for svc in SCANNED_SERVICES {
        let dir = root.join(format!("crates/{svc}/src/handler"));
        visit_rust_files(&dir, &mut |path, contents| {
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");

            let production = contents
                .find("\n#[cfg(test)]")
                .map_or(contents, |cut| &contents[..cut]);

            for fn_name in handler_fn_names(production) {
                let Some(body) = slurp_fn_body(production, &fn_name) else {
                    continue;
                };
                if !body.contains("projects::create(") {
                    continue;
                }
                let audits_a_project = parse_emit_audit_calls(&body)
                    .iter()
                    .any(|e| normalize_resource_kind(&e.resource_kind) == "Project");
                if !audits_a_project {
                    silent.push(format!("{rel}::{fn_name}"));
                }
            }
        });
    }

    silent.sort();
    assert!(
        silent.is_empty(),
        "handler(s) create a project and record nothing: {silent:?}.\n  \
         A project row that appears with no `audit.events` line is a gap nothing \
         else can reconstruct — the chain has no entry to re-verify. Emit a \
         `TelmoniResourceKind::Project` / `Created` event on the SAME transaction \
         as the insert, and add a MUTATIONS row for it."
    );
}

#[test]
fn handler_emit_set_matches_manifest() {
    let root = project_root();
    let expected: BTreeSet<(String, String)> = MUTATIONS
        .iter()
        .map(|m| (m.file.to_string(), m.fn_name.to_string()))
        .collect();

    let mut found: BTreeSet<(String, String)> = BTreeSet::new();

    let mut scan = |path: &std::path::Path, contents: &str| {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        for fn_name in handler_fn_names(contents) {
            if let Some(body) = slurp_fn_body(contents, &fn_name)
                && body.contains("emit_audit(")
            {
                found.insert((rel.clone(), fn_name));
            }
        }
    };
    for svc in SCANNED_SERVICES {
        visit_rust_files(&root.join(format!("crates/{svc}/src/handler")), &mut scan);
        // The one-shot subcommands (`terminate`, `restore`) mutate outside a
        // handler, and are audited all the same.
        let sweeps = root.join(format!("crates/{svc}/src/sweep.rs"));
        if let Ok(contents) = std::fs::read_to_string(&sweeps) {
            scan(&sweeps, &contents);
        }
    }

    let missing: Vec<_> = expected.difference(&found).collect();
    let extra: Vec<_> = found.difference(&expected).collect();

    let mut violations = Vec::new();
    for (file, fn_name) in &missing {
        violations.push(format!(
            "MISSING — `{file}::{fn_name}` is in MUTATIONS but has no `emit_audit(` call in the fn body. \
             Either the call was deleted, or the fn was renamed.",
        ));
    }
    for (file, fn_name) in &extra {
        violations.push(format!(
            "ORPHAN — `{file}::{fn_name}` calls `emit_audit(` but isn't in MUTATIONS. \
             If this is a new mutation handler, add a row to MUTATIONS in audit_coverage.rs. \
             If it's a read endpoint with an accidental audit emit, remove the call.",
        ));
    }

    assert!(
        violations.is_empty(),
        "Handler emit set differs from MUTATIONS manifest:\n  {}",
        violations.join("\n  "),
    );
}

#[derive(Debug)]
struct EmitCall {
    action: String,
    resource_kind: String,
    actor_id: String,
}

/// The body of `async fn <fn_name>` in `contents`, between matching braces.
fn slurp_fn_body(contents: &str, fn_name: &str) -> Option<String> {
    let pub_needle = format!("pub async fn {fn_name}(");
    let priv_needle = format!("async fn {fn_name}(");
    let start = contents.find(&pub_needle).or_else(|| {
        let mut search_from = 0;
        loop {
            let rel = contents[search_from..].find(&priv_needle)?;
            let abs = search_from + rel;
            let preceded_by_pub = contents
                .as_bytes()
                .get(abs.wrapping_sub(4)..abs)
                .is_some_and(|w| w == b"pub ");
            if !preceded_by_pub {
                return Some(abs);
            }
            search_from = abs + priv_needle.len();
        }
    })?;
    let after_sig = &contents[start..];
    let brace_open = after_sig.find('{')?;
    let body_start = start + brace_open + 1;
    let bytes = contents.as_bytes();
    let mut depth = 1usize;
    let mut i = body_start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(contents[body_start..i].to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Every `emit_audit(` call in a fn body, with its action, kind and actor.
fn call_end(src: &str) -> Option<usize> {
    let bytes = src.as_bytes();
    let open = src.find('(')?;
    let (mut depth, mut in_str, mut escaped) = (0i32, false, false);
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        if in_str {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_emit_audit_calls(body: &str) -> Vec<EmitCall> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(idx) = body[cursor..].find("emit_audit(") {
        let abs = cursor + idx;
        let end = call_end(&body[abs..]).map_or(body.len(), |n| abs + n);
        let window = &body[abs..end];
        let action = extract_field(window, "action").unwrap_or_default();
        let resource_kind = extract_field(window, "resource_kind").unwrap_or_default();
        let actor_id = extract_field(window, "actor").unwrap_or_default();
        out.push(EmitCall {
            action,
            resource_kind,
            actor_id,
        });
        cursor = abs + "emit_audit(".len();
    }
    out
}

/// Read the value of a struct field in an `AuditEvent { ... }` literal.
fn extract_field(window: &str, field: &str) -> Option<String> {
    let needle = format!("{field}:");
    if let Some(start) = window.find(&needle) {
        let after = &window[start + needle.len()..];
        let line_end = after.find('\n').unwrap_or(after.len());
        let line = after[..line_end].trim();
        let value = line.trim_end_matches(',').trim();
        return Some(value.to_string());
    }
    if window.contains(&format!("{field},")) {
        return Some(field.to_string());
    }
    None
}

/// Canonicalise any `…AuditAction::Created` spelling to the variant name.
fn normalize_action(raw: &str) -> String {
    raw.rsplit("::").next().unwrap_or(raw).trim().to_string()
}

fn normalize_resource_kind(raw: &str) -> String {
    raw.rsplit("::").next().unwrap_or(raw).trim().to_string()
}

/// Does the parsed `actor:` value match the expected pattern?
fn actor_matches(parsed: &str, expected: ActorPattern) -> bool {
    match expected {
        ActorPattern::Service(name) => parsed == format!("Actor::Service(\"{name}\")"),
        ActorPattern::External(name) => parsed == format!("Actor::External(\"{name}\")"),
        ActorPattern::Variable => parsed.starts_with("Actor::User(") && parsed.ends_with(')'),
        ActorPattern::Principal => parsed == "actor",
    }
}

fn describe_pattern(p: ActorPattern) -> String {
    match p {
        ActorPattern::Service(name) => format!("Actor::Service(\"{name}\")"),
        ActorPattern::External(name) => format!("Actor::External(\"{name}\")"),
        ActorPattern::Variable => "Actor::User(<session-sourced sub>)".to_string(),
        ActorPattern::Principal => "actor (the lane's principal, passed in)".to_string(),
    }
}

/// Every top-level `async fn` in a file, public or private.
fn handler_fn_names(contents: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for line in contents.lines() {
        let trimmed = line.trim_start();
        let rest = trimmed
            .strip_prefix("pub async fn ")
            .or_else(|| trimmed.strip_prefix("pub(crate) async fn "))
            .or_else(|| trimmed.strip_prefix("pub(super) async fn "))
            .or_else(|| trimmed.strip_prefix("async fn "));
        if let Some(rest) = rest {
            let name = rest
                .split('(')
                .next()
                .unwrap_or("")
                .split('<')
                .next()
                .unwrap_or("")
                .trim();
            if !name.is_empty() {
                out.insert(name.to_string());
            }
        }
    }
    out.into_iter().collect()
}

fn project_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("repo root resolves from crates/shared")
        .to_path_buf()
}

fn visit_rust_files(dir: &Path, visit: &mut dyn FnMut(&Path, &str)) {
    let Ok(entries) = fs::read_dir(dir) else {
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
            && let Ok(contents) = fs::read_to_string(&path)
        {
            visit(&path, &contents);
        }
    }
}

/// Every `emit_audit` call must propagate with `?`.
#[test]
fn every_emit_propagates_its_error() {
    let root = project_root();
    let mut violations = Vec::new();

    for svc in ["auth", "notifications", "telmoni", "shared", "migrator"] {
        let dir = root.join(format!("crates/{svc}/src"));
        visit_rust_files(&dir, &mut |path, contents| {
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");

            for (idx, _) in contents.match_indices("emit_audit(") {
                let before = &contents[..idx];
                if before.ends_with("fn ") || before.ends_with("async fn ") {
                    continue;
                }
                let line = before.matches('\n').count() + 1;

                let tail = &contents[idx..];
                let Some(aw) = tail.find(".await") else {
                    violations.push(format!("{rel}:{line} — `emit_audit` call with no `.await`"));
                    continue;
                };
                let after: String = tail[aw + ".await".len()..]
                    .chars()
                    .take_while(|c| c.is_whitespace() || *c == '?' || *c == ';')
                    .collect();
                if !after.trim_start().starts_with('?') {
                    violations.push(format!(
                        "{rel}:{line} — `emit_audit(...).await` is not followed by `?`. \
                         A swallowed audit error leaves `app.organization_id` bound on the \
                         caller's transaction; see this test's doc comment.",
                    ));
                }
            }
        });
    }

    assert!(
        violations.is_empty(),
        "audit emits that do not propagate:\n  {}",
        violations.join("\n  "),
    );
}
