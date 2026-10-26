//! The module's lanes: the console's under `/internal/v1/telemetry`, each asking
//! auth who is acting through [`telmoni_shared::seam::Auth`], since this
//! module holds no grant on auth's roster; and ingest's, `/otlp/v1/traces`,
//! which a machine reaches with a project API key auth verifies the same way.

pub(crate) mod chain;
pub mod content_mode;
pub mod field_inventory;
pub mod ingest;
pub mod rules;
pub mod session_layout;
pub mod sessions;
pub mod traces;
pub mod users;
pub mod views;
