//! The console's lanes under `/internal/telemetry`. Each asks auth who is
//! acting, through [`telmoni_shared::seam::Auth`], since this module holds no
//! grant on auth's roster.

pub mod content_mode;
