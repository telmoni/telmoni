//! The warning channel: `tracing::warn!` unless the client was built with
//! another sink, and each distinct warning once for the client's life, so a
//! platform answering the same thing on every export is heard once. Each
//! line comes with a [`Warning`], so a sink decides without parsing words.

use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Mutex;

/// How many distinct warnings are remembered before the set starts over.
const WARNED_MAX: usize = 1024;

/// What a warning says became of the spans, beside its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// What became of them.
    pub kind: WarningKind,
    /// The HTTP status an export last met; [`WarningKind::Refused`] and
    /// [`WarningKind::Failed`] alone, where an answer came.
    pub status: Option<u16>,
    /// The problem type that answer's body named, where it named one.
    pub problem_type: Option<String>,
}

impl From<WarningKind> for Warning {
    fn from(kind: WarningKind) -> Self {
        Self {
            kind,
            status: None,
            problem_type: None,
        }
    }
}

/// What became of the spans a warning is about: the same names in every SDK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WarningKind {
    /// An export refused for good, its batch dropped; or the endpoint refused,
    /// nothing sent.
    Refused,
    /// An export that failed every attempt, its batch dropped.
    Failed,
    /// Spans dropped before sending, the queue full.
    QueueFull,
    /// A batch its own export dropped, a hook having panicked.
    ExportError,
    /// A span alone past an export's 1 MiB, dropped.
    TooLarge,
    /// A span alone past 1 MiB, sent without the attributes ingest drops for
    /// their length.
    Trimmed,
    /// An attribute the server dropped, the span kept.
    Dropped,
    /// A span the server refused, the rest of the export kept.
    SpanRefused,
    /// A line the server gave naming neither.
    Said,
    /// Something the SDK was handed fixed, cut or left off, or not sent.
    Input,
}

impl WarningKind {
    /// Its name in every SDK, snake case.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Refused => "refused",
            Self::Failed => "failed",
            Self::QueueFull => "queue_full",
            Self::ExportError => "export_error",
            Self::TooLarge => "too_large",
            Self::Trimmed => "trimmed",
            Self::Dropped => "dropped",
            Self::SpanRefused => "span_refused",
            Self::Said => "said",
            Self::Input => "input",
        }
    }
}

/// Where warnings go: each line, and what it describes.
pub(crate) type Sink = dyn Fn(&str, &Warning) + Send + Sync;

pub(crate) struct Warner {
    sink: Box<Sink>,
    warned: Mutex<HashSet<String>>,
}

impl Warner {
    pub(crate) fn new(sink: Box<Sink>) -> Self {
        Self {
            sink,
            warned: Mutex::new(HashSet::new()),
        }
    }

    /// `text`, the first time `key` is seen.
    pub(crate) fn once(&self, key: String, text: &str, warning: Warning) {
        // The lock is released before the sink runs: a sink is the
        // application's, and may take its time.
        let fresh = {
            let mut warned = crate::lock(&self.warned);
            // Bounded: a server naming ever-new attributes would otherwise
            // grow it for the life of the process; past the cap, warnings may
            // repeat.
            if !warned.contains(&key) && warned.len() >= WARNED_MAX {
                warned.clear();
            }
            warned.insert(key)
        };
        if fresh {
            self.give(text, &warning);
        }
    }

    /// `text`, every time: for a count, which is never the same warning twice.
    pub(crate) fn warn(&self, text: &str, warning: Warning) {
        self.give(text, &warning);
    }

    fn give(&self, text: &str, warning: &Warning) {
        // The sink is the application's: a panic in it must not reach the agent.
        let _ = panic::catch_unwind(AssertUnwindSafe(|| (self.sink)(text, warning)));
    }
}
