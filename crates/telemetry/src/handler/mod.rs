//! The module's lanes: the console's under `/internal/v1/telemetry`, each asking
//! auth who is acting through [`telmoni_shared::seam::Auth`], since this
//! module holds no grant on auth's roster; and ingest's, `/otlp/v1/traces`,
//! which a machine reaches with a project API key auth verifies the same way.

pub mod agents;
pub(crate) mod chain;
pub mod content_mode;
pub mod field_inventory;
pub mod history;
pub mod incidents;
pub mod ingest;
pub mod mutes;
pub mod objectives;
pub mod prices;
pub mod privacy;
pub mod rules;
pub mod scores;
pub mod traces;
pub mod usage;
pub mod users;
pub mod v1;
pub mod views;

/// Refused where the project keeps no content, `why` saying what that leaves
/// undone and how to go on.
pub(crate) fn content_kept(
    mode: telmoni_shared::ContentMode,
    why: &str,
) -> Result<(), telmoni_shared::TelmoniError> {
    if mode == telmoni_shared::ContentMode::On {
        Ok(())
    } else {
        Err(telmoni_shared::TelmoniError::BadRequest(format!(
            "this project keeps no content, {why}"
        )))
    }
}
