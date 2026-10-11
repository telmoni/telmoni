//! A sampler that records every span and decides nothing of its own.
//!
//! A run takes the sampled flag of the trace it joins. Where it starts a
//! trace, it takes the decision the configured sampler gives it, so an
//! application sampling at a tenth keeps a tenth of its own spans inside runs
//! as outside them. Nothing is ever dropped here: the SDK's processor exports
//! recorded spans whether the flag is set or not, and applies the run's
//! sample rate itself.

use opentelemetry::trace::{Link, SpanContext, SpanKind, TraceContextExt as _, TraceId};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::trace::{Sampler, SamplingDecision, SamplingResult, ShouldSample};

#[derive(Clone, Debug)]
pub(crate) struct RecordingSampler {
    /// The application's sampler for a root span, when it gave one. Rust's
    /// OpenTelemetry has no global provider to read a sampler from, so without
    /// one every root is sampled, as an application's default would have it.
    root: Option<Box<dyn ShouldSample>>,
}

impl RecordingSampler {
    pub(crate) fn new(root: Option<Box<dyn ShouldSample>>) -> Self {
        Self { root }
    }
}

impl ShouldSample for RecordingSampler {
    fn should_sample(
        &self,
        parent_context: Option<&Context>,
        trace_id: TraceId,
        name: &str,
        span_kind: &SpanKind,
        attributes: &[KeyValue],
        links: &[Link],
    ) -> SamplingResult {
        let parent = parent_context
            .map(|cx| cx.span().span_context().clone())
            .filter(SpanContext::is_valid);
        if let Some(parent) = parent {
            return SamplingResult {
                decision: recording(parent.is_sampled()),
                attributes: Vec::new(),
                trace_state: parent.trace_state().clone(),
            };
        }
        let always_on = Sampler::AlwaysOn;
        let root: &dyn ShouldSample = self.root.as_deref().unwrap_or(&always_on);
        let decision =
            root.should_sample(parent_context, trace_id, name, span_kind, attributes, links);
        SamplingResult {
            decision: recording(decision.decision == SamplingDecision::RecordAndSample),
            attributes: decision.attributes,
            trace_state: decision.trace_state,
        }
    }
}

fn recording(sampled: bool) -> SamplingDecision {
    if sampled {
        SamplingDecision::RecordAndSample
    } else {
        SamplingDecision::RecordOnly
    }
}
