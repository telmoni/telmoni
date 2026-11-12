//! The Rust SDK for Telmoni, privacy-first, open-source monitoring for AI
//! agents: an agent's traces, steps, model calls, tool calls and retrievals
//! as OpenTelemetry spans, sent to Telmoni and nowhere else. The client takes
//! prompts, completions and tool payloads off every span before it leaves the
//! process unless you ask and the project allows.
//!
//! Telmoni is self-hosted (<https://github.com/telmoni/telmoni#self-hosting>):
//! `TELMONI_ENDPOINT`, or [`Config::endpoint`], names your deployment's
//! endpoint, and there is no default.
//!
//! The client owns a tracer provider of its own, built part by part so no
//! `OTEL_*` variable reaches it, and runs beside whatever OpenTelemetry the
//! application has: a trace opened while an application span is current is a
//! trace of its own, linked to that span, an application span opened under
//! [`Trace::attach`] or [`Trace::context`] nests under the agent's span, and
//! each side exports its own spans only. Both take the application on this
//! crate's `opentelemetry` minor, re-exported as [`opentelemetry`]: cargo
//! builds each 0.x minor as a crate of its own, with a current context of its
//! own.
//!
//! ```no_run
//! use telmoni::{
//!     Config, ModelCallOptions, Reply, RetrievalOptions, Telmoni, ToolCallOptions, TraceOptions, Usage,
//! };
//!
//! # fn main() -> Result<(), telmoni::Error> {
//! // Reads TELMONI_ENDPOINT, TELMONI_API_KEY, TELMONI_ORG, TELMONI_ENVIRONMENT, TELMONI_RELEASE,
//! // TELMONI_CAPTURE_CONTENT, TELMONI_SCRUB_SECRETS and TELMONI_SCRUB_PII.
//! let telmoni = Telmoni::new(Config::from_env());
//!
//! let trace = telmoni.trace(
//!     "support-bot",
//!     TraceOptions {
//!         user_id: Some("user_42".into()),
//!         max_spend: Some(0.50),
//!         ..TraceOptions::default()
//!     },
//! )?;
//! {
//!     let step = trace.step("plan")?;
//!     let call = step.model_call(
//!         "gpt-4o-mini",
//!         ModelCallOptions {
//!             provider: Some("openai".into()),
//!             ..ModelCallOptions::default()
//!         },
//!     )?;
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
//! let tool = trace.tool_call("search", ToolCallOptions::default())?;
//! tool.returned(serde_json::json!({ "results": 3 }));
//! drop(tool);
//! let found = trace.retrieval(
//!     "kb-articles",
//!     RetrievalOptions {
//!         query: Some("refunds".into()),
//!         top_k: Some(5),
//!         ..RetrievalOptions::default()
//!     },
//! )?;
//! found.returned(&[serde_json::json!({ "id": "kb-112", "score": 0.91 })]);
//! drop(found);
//! trace.end();
//!
//! telmoni.shutdown();
//! # Ok(())
//! # }
//! ```
//!
//! A guard ends its span when it is dropped or on `end()`. The `_with`
//! methods take a closure instead and return its value, or the [`Error`] that
//! kept their handle from opening; their `try_` forms, for a closure whose
//! error converts from [`Error`], mark an `Err` on the span with the error
//! type's name and return it unchanged. A step, a model call, a tool call or
//! a retrieval opened once its trace reached a cap it was opened with —
//! `max_steps`, or `max_spend` in US dollars, counted in this process — is
//! [`Error::CapExceeded`], and opens nothing.
//!
//! A trace that has already ended — a job's own log, a hook that runs after
//! the turn it reports — is sent whole from its [`TraceRecord`] by
//! [`Telmoni::record_trace`].

#![warn(missing_docs)]

mod client;
mod config;
mod contract;
mod error;
mod exporter;
mod guard;
mod ids;
mod processor;
mod propagation;
mod records;
mod sampler;
mod scores;
// The server scans other distributions' content with these; no part of the SDK's API.
#[doc(hidden)]
pub mod scrub;
mod traces;
pub mod transport;
mod warnings;

pub use client::{Hooks, Telmoni};
pub use config::{Config, Mask, Price};
pub use error::{Cap, Error, Text};
pub use exporter::ContentMode;
pub use processor::Batching;
pub use propagation::Carrier;
pub use records::{
    CallRecord, ModelCallRecord, RecordItem, StepRecord, ToolCallRecord, TraceRecord,
};
pub use scores::{Score, ScoreTarget};
pub use traces::{
    Content, ModelCall, ModelCallOptions, Reply, Retrieval, RetrievalOptions, Step, ToolCall,
    ToolCallOptions, Trace, TraceOptions, TraceReply, Usage,
};
pub use warnings::{Warning, WarningKind};

/// The OpenTelemetry a trace is linked to and hands its context to: nest your
/// own spans under a trace through it, or through the same `opentelemetry`
/// minor.
pub use opentelemetry;
/// Its SDK, whose `ShouldSample` [`Config::sampler`] takes.
pub use opentelemetry_sdk;

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
