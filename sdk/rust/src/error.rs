//! What the SDK refuses: what it cannot make a run of, and what a run opens
//! past a cap it was opened with.

use std::fmt;

/// Why a run did not open, a record was not sent, or a step or call did not
/// open. Nothing else in the SDK fails: a step, a model call or a tool call
/// opens unless its run reached a cap, and an export that fails is warned
/// about, never returned.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// The agent's name was blank; a run is counted under its agent.
    EmptyAgent,
    /// The sample rate, given per run or in the configuration, was not above 0
    /// and up to 1.
    SampleRate(f64),
    /// A run's `max_steps` was 0, a cap no step fits under.
    MaxSteps,
    /// A run's `max_spend` was not a finite number of US dollars above 0.
    MaxSpend(f64),
    /// The run reached a cap it was opened with, so what it would open next
    /// opens nothing.
    CapExceeded {
        /// Which cap.
        cap: Cap,
        /// The cap.
        limit: f64,
        /// What the run had used of it: its steps opened, or the US dollars
        /// it spent.
        used: f64,
    },
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
    /// The client was shut down, so nothing of the record was queued.
    Shutdown,
}

/// A run's cap, which [`Error::CapExceeded`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Cap {
    /// `max_steps`: the run's steps.
    Steps,
    /// `max_spend`: what its model calls cost.
    Spend,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyAgent => f.write_str("a run needs its agent's name"),
            Self::SampleRate(rate) => {
                write!(f, "sample_rate must be above 0 and up to 1, not {rate}")
            }
            Self::MaxSteps => f.write_str("max_steps must be a whole number above 0"),
            Self::MaxSpend(spend) => write!(
                f,
                "max_spend must be a finite number of US dollars above 0, not {spend}"
            ),
            Self::CapExceeded {
                cap: Cap::Steps,
                limit,
                ..
            } => write!(f, "the run reached its step cap of {limit}"),
            Self::CapExceeded {
                cap: Cap::Spend,
                limit,
                used,
            } => write!(
                f,
                "the run reached its spend cap of ${limit}, having spent ${used}"
            ),
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
            Self::Shutdown => f.write_str("the client is shut down; the record was not sent"),
        }
    }
}

impl std::error::Error for Error {}
