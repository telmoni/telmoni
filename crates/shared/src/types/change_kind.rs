//! [`ChangeKind`] — what an agent's change changed.

use serde::{Deserialize, Serialize};

/// What changed in an agent's observed behavior. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// The release its spans were sent from, `service.version`.
    Release,
    /// The model a model call answered with.
    Model,
    /// A prompt's version, under its name.
    Prompt,
    /// A tool or retrieval source first called within remembered history.
    Tool,
}

crate::wire_enum!(ChangeKind {
    Release => "release",
    Model => "model",
    Prompt => "prompt",
    Tool => "tool",
});
