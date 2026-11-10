//! [`ChangeKind`] — what an agent's change changed.

use serde::{Deserialize, Serialize};

/// What changed: the release its traces were sent from, a model call's model,
/// or a prompt's version. `[PUBLIC-API]`.
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
}

crate::wire_enum!(ChangeKind {
    Release => "release",
    Model => "model",
    Prompt => "prompt",
});
