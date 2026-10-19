//! What the SDK refuses: only what it cannot make a run of.

use std::fmt;

/// Why a run did not open, or a record was not sent. Nothing else in the SDK
/// fails: a step, a model call or a tool call always opens, and an export
/// that fails is warned about, never returned.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// The agent's name was blank; a run is counted under its agent.
    EmptyAgent,
    /// The sample rate, given per run or in the configuration, was not above 0
    /// and up to 1.
    SampleRate(f64),
    /// A recorded run's id was not a UUID, or was the nil UUID: its trace's id
    /// and its spans' are derived from it, and a trace id of zeros is invalid.
    RunId,
    /// A recorded span — the run's own, a step's or a call's — ended before
    /// it started.
    EndBeforeStart,
    /// A recorded time fell at or before the Unix epoch, or 2^64 nanoseconds
    /// or more after it, in 2554: ingest reads neither.
    RecordedTime,
    /// A recorded step's name or model call's model was empty, which ingest
    /// refuses the span for.
    EmptyName,
    /// A recorded text held more characters than ingest takes there, which it
    /// refuses the span for.
    TooLong {
        /// Which text, never the text itself: `"step's name"`, …
        what: &'static str,
        /// The most characters ingest takes there.
        max: usize,
    },
    /// The export queue had no room for the whole record, so nothing of it
    /// was queued: flush, or wait for the queue to drain, and record it
    /// again.
    QueueFull {
        /// How many spans the record would have queued.
        spans: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyAgent => f.write_str("a run needs its agent's name"),
            Self::SampleRate(rate) => {
                write!(f, "sample_rate must be above 0 and up to 1, not {rate}")
            }
            Self::RunId => {
                f.write_str("a recorded run's id must be a UUID other than the nil UUID")
            }
            Self::EndBeforeStart => f.write_str("a recorded span cannot end before it starts"),
            Self::RecordedTime => f.write_str(
                "a recorded time must fall after the Unix epoch and less than 2^64 nanoseconds after it",
            ),
            Self::EmptyName => {
                f.write_str("a recorded step's name or model call's model cannot be empty")
            }
            Self::TooLong { what, max } => {
                write!(f, "a recorded {what} must be up to {max} characters")
            }
            Self::QueueFull { spans } => write!(
                f,
                "the export queue has no room for the record's {spans} spans; flush or wait, then record it again"
            ),
        }
    }
}

impl std::error::Error for Error {}
