//! The Rust SDK for Telmoni: an agent's runs, steps, model calls and tool
//! calls as OpenTelemetry spans, sent to Telmoni and nowhere else.
//!
//! The client owns a tracer provider of its own, built part by part so no
//! `OTEL_*` variable reaches it, and runs beside whatever OpenTelemetry the
//! application has: a run opened while an application span is current joins
//! that span's trace, an application span opened under [`Run::attach`] or
//! [`Run::context`] nests under the run, and each side exports its own spans
//! only.
//!
//! ```no_run
//! use telmoni_sdk::{Config, ModelCallOptions, Reply, RunOptions, Telmoni, ToolCallOptions, Usage};
//!
//! # fn main() -> Result<(), telmoni_sdk::Error> {
//! // Reads TELMONI_ENDPOINT, TELMONI_API_KEY and TELMONI_ORG.
//! let telmoni = Telmoni::new(Config::from_env());
//!
//! let run = telmoni.run(
//!     "support-bot",
//!     RunOptions {
//!         user_id: Some("user_42".into()),
//!         ..RunOptions::default()
//!     },
//! )?;
//! {
//!     let step = run.step("plan");
//!     let call = step.model_call(
//!         "gpt-4o-mini",
//!         ModelCallOptions {
//!             provider: Some("openai".into()),
//!             ..ModelCallOptions::default()
//!         },
//!     );
//!     call.answered(Reply {
//!         response_model: Some("gpt-4o-mini-2024-07-18".into()),
//!         usage: Some(Usage {
//!             input_tokens: Some(12),
//!             output_tokens: Some(7),
//!             ..Usage::default()
//!         }),
//!         ..Reply::default()
//!     });
//! }
//! let tool = run.tool_call("search", ToolCallOptions::default());
//! tool.returned(serde_json::json!({ "results": 3 }));
//! drop(tool);
//! run.end();
//!
//! telmoni.shutdown();
//! # Ok(())
//! # }
//! ```
//!
//! A guard ends its span when it is dropped or on `end()`. The `_with`
//! methods take a closure instead and return its value; their `try_` forms
//! mark an `Err` on the span with the error type's name and return it
//! unchanged.
//!
//! A run that has already ended — a job's own log, a hook that runs after the
//! turn it reports — is sent whole from its [`RunRecord`] by
//! [`Telmoni::record_run`].

#![warn(missing_docs)]

mod client;
mod config;
mod contract;
mod error;
mod exporter;
mod ids;
mod processor;
mod propagation;
mod records;
mod runs;
mod sampler;
pub mod transport;
mod warnings;

pub use client::{Hooks, Telmoni};
pub use config::{Config, DEFAULT_ENDPOINT};
pub use error::Error;
pub use processor::Batching;
pub use propagation::Carrier;
pub use records::{CallRecord, ModelCallRecord, RecordItem, RunRecord, StepRecord, ToolCallRecord};
pub use runs::{
    Bounds, Content, ModelCall, ModelCallOptions, Reply, Run, RunOptions, Step, ToolCall,
    ToolCallOptions, Usage,
};

/// The SDK's version: the crate's, sent as `telemetry.distro.version`, as the
/// scope's version and in `User-Agent`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A lock whatever a panic elsewhere left behind: nothing under a lock here is
/// left half-written at a release, so a poisoned one is as good as a clean one.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
