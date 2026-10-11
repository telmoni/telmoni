//! What the SDK refuses: only what it cannot make a run of.

use std::fmt;

/// Why a run did not open. Nothing else in the SDK fails: a step, a model
/// call or a tool call always opens, and an export that fails is warned
/// about, never returned.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// The agent's name was blank; a run is counted under its agent.
    EmptyAgent,
    /// The sample rate, given per run or in the configuration, was not above 0
    /// and up to 1.
    SampleRate(f64),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyAgent => f.write_str("a run needs its agent's name"),
            Self::SampleRate(rate) => {
                write!(f, "sample_rate must be above 0 and up to 1, not {rate}")
            }
        }
    }
}

impl std::error::Error for Error {}
