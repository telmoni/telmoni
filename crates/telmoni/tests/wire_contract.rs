//! Wire-contract pin: every vocabulary the console shares with the server,
//! from each module this binary links, as `contract/wire-contract.json`
//! holds it for the console's `contract.test.ts`.

use std::path::PathBuf;

use serde_json::{Value, json};
use telmoni_agent::tools::Tool;
use telmoni_auth::audit_export::{ExportFailure, ExportFormat, ExportStatus};
use telmoni_auth::model::InviteScope;
use telmoni_notifications::connector::{ConnectionStatus, Provider};
use telmoni_notifications::db::{AttemptOutcome, AttemptTrigger, DeliveryStatus};
use telmoni_shared::types::WireEnum;
use telmoni_shared::{
    ApiKeyScope, ContentMode, Flag, IncidentStatus, LayoutDepth, LayoutOpen, MissingData,
    NotificationKind, OrganizationRole, OrganizationStatus, Role, RuleAggregation, RuleLevel,
    RuleMeasure, RuleOperator, RuleWindow, SessionSort, Severity, SortDirection, SpanKind,
    SpanStatus, TelmoniResourceKind, TraceColumn, TraceSort, UserSort, ViewKind, slug,
};
use telmoni_telemetry::live;
use telmoni_telemetry::rules::db::PausedBy;

/// A vocabulary's words in its table's order, each the one serde writes
/// (`telmoni_shared::test_util::assert_wire_words` holds the two together).
fn wire<T: WireEnum>() -> Vec<&'static str> {
    T::variants().iter().map(|variant| variant.word()).collect()
}

/// The contract as the live vocabularies define it.
fn generated_contract() -> Value {
    json!({
        "$generated": "by `make contract` from the modules' vocabularies and slug.rs — do not edit by hand",
        "enums": {
            "Role": wire::<Role>(),
            "OrganizationRole": wire::<OrganizationRole>(),
            "OrganizationStatus": wire::<OrganizationStatus>(),
            "NotificationKind": wire::<NotificationKind>(),
            "SpanKind": wire::<SpanKind>(),
            "SpanStatus": wire::<SpanStatus>(),
            "TraceSort": wire::<TraceSort>(),
            "TraceColumn": wire::<TraceColumn>(),
            "UserSort": wire::<UserSort>(),
            "SessionSort": wire::<SessionSort>(),
            "SortDirection": wire::<SortDirection>(),
            "ContentMode": wire::<ContentMode>(),
            "ApiKeyScope": wire::<ApiKeyScope>(),
            "LayoutDepth": wire::<LayoutDepth>(),
            "LayoutOpen": wire::<LayoutOpen>(),
            "ViewKind": wire::<ViewKind>(),
            "RuleMeasure": wire::<RuleMeasure>(),
            "RuleAggregation": wire::<RuleAggregation>(),
            "RuleWindow": wire::<RuleWindow>(),
            "RuleOperator": wire::<RuleOperator>(),
            "MissingData": wire::<MissingData>(),
            "Severity": wire::<Severity>(),
            "IncidentStatus": wire::<IncidentStatus>(),
            "RuleLevel": wire::<RuleLevel>(),
            "Flag": wire::<Flag>(),
            "ResourceKind": wire::<TelmoniResourceKind>(),
            "PausedBy": wire::<PausedBy>(),
            "ConnectorProvider": wire::<Provider>(),
            "ConnectionStatus": wire::<ConnectionStatus>(),
            "DeliveryStatus": wire::<DeliveryStatus>(),
            "AttemptTrigger": wire::<AttemptTrigger>(),
            "AttemptOutcome": wire::<AttemptOutcome>(),
            "ExportFormat": wire::<ExportFormat>(),
            "ExportStatus": wire::<ExportStatus>(),
            "ExportFailure": wire::<ExportFailure>(),
            "InviteScope": wire::<InviteScope>(),
            "AgentTool": Tool::ALL.map(Tool::name),
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
        "live": {
            "organization_channel": live::ORGANIZATION_CHANNEL,
            "spans_ingested": live::SPANS_INGESTED,
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
