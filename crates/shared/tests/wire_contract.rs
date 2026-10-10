//! Wire-contract pin — the cross-language source of truth for enum wire vocab.
#![expect(
    clippy::expect_used,
    reason = "test scaffolding: asserts and fixture setup"
)]

use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value, json};
use telmoni_shared::{
    ContentMode, Flag, NotificationKind, OrganizationStatus, Role, SpanKind, SpanStatus, slug,
};

/// Serialize each variant to its wire string, per the enum's `#[serde(rename_all)]`.
fn wire<T: Serialize>(variants: &[T]) -> Vec<String> {
    variants
        .iter()
        .map(|v| {
            serde_json::to_value(v)
                .expect("enum serializes")
                .as_str()
                .expect("enum serializes to a string")
                .to_owned()
        })
        .collect()
}

/// Every variant of a fieldless enum, once; the generated `match` stops compiling
/// when a variant is added and not listed here.
macro_rules! all_variants {
    ($ty:ty; $($variant:ident),+ $(,)?) => {{
        let all = [$(<$ty>::$variant),+];
        for v in &all {
            match v {
                $(<$ty>::$variant => {}),+
            }
        }
        all
    }};
}

/// The contract as the live enums define it.
fn generated_contract() -> Value {
    json!({
        "$generated": "by `make contract` from crates/shared/src/types and slug.rs — do not edit by hand",
        "enums": {
            "Role": wire(&all_variants!(Role; Owner, Admin, Member)),
            "OrganizationStatus": wire(&all_variants!(OrganizationStatus; Active, PendingDeletion, Deleted)),
            "NotificationKind": wire(&NotificationKind::all()),
            "SpanKind": wire(&SpanKind::all()),
            "SpanStatus": wire(&SpanStatus::all()),
            "ContentMode": wire(&ContentMode::all()),
            "Flag": wire(&all_variants!(Flag; Connectors, PublicApi, ApiTokens,
                Members, Signup)),
        },
        "flags": {
            "global_only": Flag::all().iter().filter(|f| f.is_global_only())
                .map(|f| f.as_str()).collect::<Vec<_>>(),
            "off_detail": Flag::all().iter()
                .map(|f| (f.as_str().to_owned(), json!(f.off_detail())))
                .collect::<serde_json::Map<_, _>>(),
        },
        "slugs": {
            "max_length": slug::MAX_LEN,
            "reserved": {
                "organization": slug::ORGANIZATION_RESERVED,
                "project": slug::PROJECT_RESERVED,
            },
        },
    })
}

fn contract_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contract/wire-contract.json")
}

#[test]
fn wire_contract_is_in_sync() {
    let generated = generated_contract();
    let path = contract_path();

    if std::env::var("UPDATE_CONTRACT").is_ok() {
        let pretty = serde_json::to_string_pretty(&generated).expect("serialize contract");
        std::fs::write(&path, format!("{pretty}\n")).expect("write contract");
        return;
    }

    let on_disk: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .expect("read contract/wire-contract.json — run `make contract` to generate it"),
    )
    .expect("contract/wire-contract.json is valid JSON");

    assert_eq!(
        on_disk, generated,
        "contract/wire-contract.json is stale — run `make contract` to regenerate"
    );
}
