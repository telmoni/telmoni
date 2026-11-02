//! The module's lanes: the console's under `/internal/v1/telemetry`, each asking
//! auth who is acting through [`telmoni_shared::seam::Auth`], since this
//! module holds no grant on auth's roster; and ingest's, `/otlp/v1/traces`,
//! which a machine reaches with a project API key auth verifies the same way.

pub(crate) mod chain;
pub mod content_mode;
pub mod evaluation_rules;
pub mod evaluators;
pub mod field_inventory;
pub mod health;
pub mod incidents;
pub mod ingest;
pub mod model_connections;
pub mod mutes;
pub mod prices;
pub mod privacy;
pub mod rules;
pub mod scores;
pub mod session_layout;
pub mod sessions;
pub mod traces;
pub mod usage;
pub mod users;
pub mod v1;
pub mod views;
