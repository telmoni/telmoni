//! The module's lanes: the console's under `/internal/telemetry`, each asking
//! auth who is acting through [`telmoni_shared::seam::Auth`], since this
//! module holds no grant on auth's roster; and ingest's, `/otlp/v1/traces`,
//! which a machine reaches with a project API key auth verifies the same way;
//! and the ping lane's, `/ping/{ping_key}/{slug}`, which a crontab reaches
//! with the project's ping key in the URL and nothing else.

mod chain;
pub mod content_mode;
pub mod field_inventory;
pub mod ingest;
pub mod monitors;
pub mod ping_keys;
pub mod pings;
pub mod sessions;
pub mod traces;
pub mod users;
pub mod views;
