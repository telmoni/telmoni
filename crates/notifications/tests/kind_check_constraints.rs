//! `notifications.feed`, `notifications.deliveries` and a webhook's chosen
//! kinds must accept exactly the kinds Rust can emit: a drift is a 500 at the
//! first emit, or at the first choice. Every other column read back through
//! an enum — a provider, a status, a send's trigger and outcome — must admit
//! exactly the words its enum spells.
#![expect(
    clippy::indexing_slicing,
    clippy::string_slice,
    reason = "a panicking helper is a failing test"
)]
#![expect(clippy::expect_used, reason = "test scaffolding")]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use telmoni_notifications::connector::{
    ConnectionStatus, Connectors, Provider, discord, slack, webhook,
};
use telmoni_notifications::db::{AttemptOutcome, AttemptTrigger, DeliveryStatus};
use telmoni_shared::{NotificationKind, Redacted};

/// The crate's single pre-launch migration.
fn initial_migration() -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut found: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("notifications migrations directory is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_initial.sql"))
        })
        .collect();
    found.sort();

    assert_eq!(
        found.len(),
        1,
        "expected exactly one *_initial.sql in {}, found {found:?} — one per crate before v0.1.0",
        dir.display()
    );
    fs::read_to_string(&found[0]).expect("migration is readable")
}

/// Pull the parenthesised body of a named CHECK constraint.
fn find_check_body<'a>(sql: &'a str, constraint: &str) -> Option<&'a str> {
    let start = sql.find(constraint)?;
    let rest = &sql[start..];
    let open = rest.find('(')?;

    let mut depth = 0usize;
    for (offset, ch) in rest[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&rest[open..=open + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The asserting wrapper every test goes through.
fn check_body<'a>(sql: &'a str, constraint: &str) -> &'a str {
    let body = find_check_body(sql, constraint);
    assert!(
        body.is_some(),
        "constraint `{constraint}` is absent from the migration (or has no balanced paren body)"
    );
    body.unwrap_or_default()
}

/// Every `'quoted'` literal in a CHECK body.
fn literals(body: &str) -> Vec<&str> {
    body.split('\'').skip(1).step_by(2).collect()
}

/// The words a CHECK admits.
fn admitted(sql: &str, constraint: &str) -> BTreeSet<String> {
    literals(check_body(sql, constraint))
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// The words an enum spells.
fn spelled<T: ToString>(values: impl IntoIterator<Item = T>) -> BTreeSet<String> {
    values.into_iter().map(|value| value.to_string()).collect()
}

/// The kind-bearing columns: the feed, the delivery queue, and the kinds a
/// webhook chose — a kind missing there makes choosing it a 500.
const CONSTRAINTS: [&str; 3] = [
    "feed_kind_check",
    "deliveries_kind_check",
    "connections_event_kinds_check",
];

#[test]
fn the_recording_tables_accept_every_kind_variant() {
    let sql = initial_migration();
    for constraint in CONSTRAINTS {
        let body = check_body(&sql, constraint);
        for kind in NotificationKind::all() {
            let literal = format!("'{kind}'");
            assert!(
                body.contains(&literal),
                "`{kind}` is a NotificationKind variant but {constraint} rejects it — \
                 an alert the customer was owed dies at the INSERT. CHECK body: {body}"
            );
        }
    }
}

#[test]
fn the_check_admits_no_kind_the_enum_lacks() {
    let sql = initial_migration();
    let known: Vec<String> = NotificationKind::all()
        .iter()
        .map(ToString::to_string)
        .collect();

    for constraint in CONSTRAINTS {
        let body = check_body(&sql, constraint);
        for literal in literals(body) {
            assert!(
                known.iter().any(|k| k == literal),
                "{constraint} admits `{literal}`, which is not a NotificationKind. \
                 `kind` is TEXT read back through the enum, so a row stored under that \
                 spelling makes every later feed read fail to deserialize — for the \
                 whole organization, not just that row."
            );
        }
    }
}

/// Kinds only a service beside this repository raises, writing the title and
/// body itself: the emit lane is their producer's door, and nothing here
/// produces them by design. Exact, so a producer added here later takes its
/// kind off this list rather than hiding behind it.
const RAISED_BESIDE_THIS_REPOSITORY: &[NotificationKind] = &[NotificationKind::OrganizationAlert];

#[test]
fn every_kind_has_a_producer_that_raises_it() {
    use std::fs;
    use std::path::PathBuf;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ is the parent of this crate")
        .to_path_buf();

    let mut sources = Vec::new();
    for entry in fs::read_dir(&root).expect("crates/ is readable") {
        let path = entry.expect("dir entry").path().join("src/notify.rs");
        if path.is_file() {
            sources.push((
                path.clone(),
                fs::read_to_string(&path).expect("notify.rs is readable"),
            ));
        }
    }
    assert!(
        !sources.is_empty(),
        "no crates/*/src/notify.rs found — this gate would pass vacuously. \
         If the emitters moved, move this with them."
    );

    for kind in NotificationKind::all() {
        let quoted = format!("\"{kind}\"");
        let variant = format!("NotificationKind::{kind:?}");
        let produced = sources.iter().any(|(_, body)| {
            body.lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .any(|l| l.contains(&variant) || l.contains(&quoted))
        });
        if RAISED_BESIDE_THIS_REPOSITORY.contains(&kind) {
            assert!(
                !produced,
                "`{kind}` is raised in this repository now: take it off \
                 RAISED_BESIDE_THIS_REPOSITORY."
            );
            continue;
        }
        assert!(
            produced,
            "`{kind}` is in the vocabulary and NO crate emits it. A kind with \
             no producer is a switch that can never fire and a docs row that \
             is not true."
        );
    }
}

/// Exact both ways: a word only the enum spells is a write the CHECK refuses,
/// and a word only the CHECK admits is a row that fails every read decoding
/// it, the delivery loop's whole batch included.
#[test]
fn each_vocabulary_check_admits_exactly_its_enum() {
    let sql = initial_migration();
    let pins = [
        ("connections_provider_check", spelled(Provider::all())),
        ("connections_status_check", spelled(ConnectionStatus::all())),
        ("deliveries_status_check", spelled(DeliveryStatus::all())),
        (
            "delivery_attempts_trigger_check",
            spelled(AttemptTrigger::all()),
        ),
        (
            "delivery_attempts_outcome_check",
            spelled(AttemptOutcome::all()),
        ),
    ];
    for (constraint, words) in pins {
        assert_eq!(
            admitted(&sql, constraint),
            words,
            "{constraint} and its enum spell different words"
        );
    }
}

/// A handshake state is minted only for a provider with an install dialog,
/// which the webhook, connected by its URL, does not have.
#[test]
fn the_handshake_check_admits_exactly_the_providers_with_a_grant() {
    let every_connector = Connectors {
        slack: Some(Arc::new(slack::SlackConnector::new(
            "client",
            Redacted::from("secret"),
            "http://localhost",
        ))),
        discord: Some(Arc::new(discord::DiscordConnector::new(
            "client",
            Redacted::from("secret"),
            "http://localhost",
        ))),
        webhook: Some(Arc::new(webhook::WebhookConnector)),
    };
    let with_a_grant = spelled(
        Provider::all()
            .into_iter()
            .filter(|provider| every_connector.grant(*provider).is_some()),
    );
    assert_eq!(
        admitted(&initial_migration(), "oauth_states_provider_check"),
        with_a_grant,
        "oauth_states_provider_check must admit exactly the providers with a grant"
    );
}
