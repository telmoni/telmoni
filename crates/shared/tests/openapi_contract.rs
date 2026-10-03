//! `contract/openapi.json` — the public API, generated and pinned.

use std::path::PathBuf;

use serde_json::Value;
use telmoni_shared::openapi::{Answer, Method, Route, document};

/// The refusals every `/v1` lane makes because of what it is, merged into every operation.
const UNIVERSAL: &[Answer] = &[
    Answer {
        status: 401,
        description: "No live `telmoni_` token was presented. A revoked, expired \
                      or unknown token answers the same way.",
    },
    Answer {
        status: 429,
        description: "Too many requests. The front door meters by source and \
                      by token before the request reaches a service, and says \
                      when to come back in `Retry-After`.",
    },
    Answer {
        status: 503,
        description: "The lane is switched off, or something it needs did not \
                      answer. A switched-off lane says when to try again in \
                      `Retry-After`.",
    },
];

const TITLE: &str = "Telmoni API";
/// `/v1` is additive only, so this is `1` until a removal forces a `/v2`.
const VERSION: &str = "1";
const SERVER: &str = "https://telmoni.com";

/// Every lane, from the services that serve them.
fn lanes() -> Vec<Route> {
    telmoni_auth::handler::v1::V1_LANES
        .iter()
        .map(|l| l.route)
        .collect()
}

fn contract_path() -> PathBuf {
    repo_root().join("contract/openapi.json")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn openapi_contract_is_in_sync() {
    let generated = document(TITLE, VERSION, SERVER, UNIVERSAL, &lanes());
    let path = contract_path();

    if std::env::var("UPDATE_CONTRACT").is_ok() {
        let pretty = serde_json::to_string_pretty(&generated).expect("serialize the document");
        std::fs::write(&path, format!("{pretty}\n")).expect("write the document");
        return;
    }

    let on_disk: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .expect("read contract/openapi.json — run `make contract` to generate it"),
    )
    .expect("contract/openapi.json is valid JSON");

    assert_eq!(
        on_disk, generated,
        "contract/openapi.json is stale — run `make contract` to regenerate"
    );
}

/// Every path is one a customer can type at the front door.
#[test]
fn every_lane_is_reachable_through_the_front_door() {
    let lanes = lanes();
    assert!(!lanes.is_empty(), "the table did not load");
    for route in &lanes {
        assert!(
            route.path.starts_with("/v1/"),
            "{}: a lane outside /v1 is not on the public API",
            route.path
        );
        assert!(
            !route.path.ends_with('/'),
            "{}: a trailing slash is a second address for one lane",
            route.path
        );
    }
}

/// Every write on an `auth` lane is one the BFF's own table admits.
#[test]
fn every_auth_write_is_one_the_front_door_admits() {
    let door = repo_root().join("web/app/v1/[...path]/route.ts");
    let source = std::fs::read_to_string(&door).expect("read the BFF's /v1 route");
    let table = source
        .split_once("const AUTH_WRITES")
        .expect("the BFF names its write table AUTH_WRITES")
        .1
        .split_once('{')
        .expect("the write table is an object literal")
        .1
        .split_once("};")
        .expect("the write table is closed")
        .0;

    let writes: Vec<Route> = lanes()
        .into_iter()
        .filter(|r| !matches!(r.method, Method::Get | Method::Head))
        .collect();
    for route in &writes {
        let lane = route.path.trim_start_matches("/v1/");
        let method = route.method.as_str().to_uppercase();
        let entry = [
            format!("\"{lane}\": \"{method}\""),
            format!("{lane}: \"{method}\""),
        ];
        assert!(
            entry.iter().any(|e| table.contains(e.as_str())),
            "{} {}: the front door has no AUTH_WRITES entry, so it answers 405 before the hop.\n\
             Its table is:{table}",
            method,
            route.path
        );
    }

    let entries = table
        .lines()
        .filter(|l| {
            let l = l.trim();
            !l.is_empty() && !l.starts_with("//") && l.contains(':')
        })
        .count();
    assert_eq!(
        entries,
        writes.len(),
        "AUTH_WRITES holds {entries} entries for {} write lanes — the front door \n\
         forwards a method no `/v1` lane serves.\nIts table is:{table}",
        writes.len()
    );
}

/// One address, one description: a duplicate would overwrite silently and panic the router.
#[test]
fn no_two_lanes_share_a_path_and_a_method() {
    let lanes = lanes();
    let mut seen: Vec<(&str, Method)> = lanes.iter().map(|r| (r.path, r.method)).collect();
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();
    assert_eq!(before, seen.len(), "two lanes claim one address and method");
}

/// `/v1` is additive only and has no update, so PUT and PATCH have nothing to mean.
#[test]
fn nothing_under_v1_is_updated_in_place() {
    for route in lanes() {
        assert!(
            matches!(
                route.method,
                Method::Get | Method::Head | Method::Post | Method::Delete
            ),
            "{}: /v1 has no update",
            route.path
        );
    }
}

/// A `HEAD` lane must shadow a `GET` lane at the same address.
#[test]
fn every_head_lane_has_the_get_it_mirrors() {
    let lanes = lanes();
    for route in lanes.iter().filter(|r| r.method == Method::Head) {
        assert!(
            lanes
                .iter()
                .any(|r| r.method == Method::Get && r.path == route.path),
            "{}: HEAD with no GET beside it",
            route.path
        );
    }
}

/// Prose a customer reads: a sentence, never an identifier from our schema.
#[test]
fn every_lane_reads_as_customer_text() {
    for route in lanes() {
        let summary = route.summary;
        assert!(
            summary.chars().next().is_some_and(char::is_uppercase),
            "{}: a summary starts with a capital",
            route.path
        );
        assert!(
            !summary.ends_with('.'),
            "{summary}: a summary is a phrase for a list, not a sentence"
        );
        let description = route.description;
        assert!(
            description.ends_with('.'),
            "{}: a description is a sentence",
            route.path
        );
        assert!(
            !description.contains("project_id"),
            "{}: a column name is not customer text",
            route.path
        );
        for answer in route.answers {
            assert!(
                answer.description.ends_with('.'),
                "{} {}: an answer is a sentence",
                route.path,
                answer.status
            );
        }
    }
}
