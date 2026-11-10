//! The SDK beside an application's own OpenTelemetry: its spans stay in their
//! traces at their own sampling rate, a trace opened inside a span it does
//! not sample is still sent, and its `OTEL_*` variables change nothing of
//! ours.

mod common;

use std::collections::HashSet;
use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll, Waker};

use common::{
    Harness, Outcome, attributes_of, field, in_child, is_child, keyed, lock, name_of, named, only,
    starting_with, tags,
};
use opentelemetry::trace::{
    FutureExt as _, Link, Span as _, SpanKind, TraceContextExt as _, TraceId, Tracer as _,
    TracerProvider as _,
};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{
    Sampler, SamplingDecision, SamplingResult, SdkTracerProvider, ShouldSample, SpanData,
    SpanExporter,
};
use serde_json::json;
use telmoni::{Config, TraceOptions};

/// The application's exporter: what its provider sent, kept in memory.
#[derive(Debug, Clone, Default)]
struct Memory {
    spans: Arc<Mutex<Vec<SpanData>>>,
}

impl Memory {
    fn finished(&self) -> Vec<SpanData> {
        lock(&self.spans).clone()
    }
}

impl SpanExporter for Memory {
    fn export(&self, batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
        lock(&self.spans).extend(batch);
        std::future::ready(Ok(()))
    }
}

/// Samples every second root span: a rate of one half, deterministic. Clones
/// share the count, as the one sampler given to both providers.
#[derive(Debug, Clone, Default)]
struct EveryOther {
    calls: Arc<AtomicUsize>,
}

impl ShouldSample for EveryOther {
    fn should_sample(
        &self,
        _parent_context: Option<&Context>,
        _trace_id: TraceId,
        _name: &str,
        _span_kind: &SpanKind,
        _attributes: &[KeyValue],
        _links: &[Link],
    ) -> SamplingResult {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        SamplingResult {
            decision: if call.is_multiple_of(2) {
                SamplingDecision::RecordAndSample
            } else {
                SamplingDecision::Drop
            },
            attributes: Vec::new(),
            trace_state: Default::default(),
        }
    }
}

/// The application's OpenTelemetry: its own provider, exporting to `memory`,
/// sampling as an application does, `ParentBased` over `root`.
fn application(root: impl ShouldSample + 'static, memory: &Memory) -> SdkTracerProvider {
    SdkTracerProvider::builder()
        .with_simple_exporter(memory.clone())
        .with_sampler(Sampler::ParentBased(Box::new(root)))
        .build()
}

/// A trace opened while an application span is current is a trace of its
/// own, linked to that span; the application's span opened under it nests
/// in ours.
#[test]
fn a_trace_is_its_own_linked_to_the_applications_span() -> Outcome {
    let memory = Memory::default();
    let provider = application(Sampler::AlwaysOn, &memory);
    let app = provider.tracer("the-application");
    let harness = Harness::new()?;

    let request = Context::current_with_span(app.start("request"));
    let request_trace = request.span().span_context().trace_id().to_string();
    let request_span = request.span().span_context().span_id().to_string();
    {
        let _current = request.clone().attach();
        let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
        {
            let _inside = trace.attach();
            let mut query = app.start("db.query");
            query.end();
        }
        trace.end();
    }
    request.span().end();
    harness.flush()?;

    let spans = harness.spans()?;
    let agent = only(&spans, "invoke_agent ")?;
    assert_ne!(agent.get("traceId"), Some(&json!(request_trace)));
    assert!(agent.get("parentSpanId").is_none(), "a trace of its own");
    assert_eq!(
        agent.get("links"),
        Some(&json!([{ "traceId": request_trace, "spanId": request_span }])),
        "linked to the application's span"
    );
    let app_spans = memory.finished();
    let mut names: Vec<&str> = app_spans.iter().map(|span| span.name.as_ref()).collect();
    names.sort_unstable();
    assert_eq!(names, ["db.query", "request"]);
    let query = app_spans
        .iter()
        .find(|span| span.name == "db.query")
        .ok_or("db.query")?;
    assert_eq!(
        agent.get("spanId"),
        Some(&json!(query.parent_span_id.to_string())),
        "nested under the agent's span"
    );
    assert_eq!(
        agent.get("traceId"),
        Some(&json!(query.span_context.trace_id().to_string()))
    );
    assert!(
        !spans
            .iter()
            .any(|span| matches!(name_of(span), "request" | "db.query")),
        "the application's spans never reach Telmoni"
    );
    provider.shutdown()?;
    Ok(())
}

#[test]
fn the_application_keeps_its_own_sampling_rate_inside_traces() -> Outcome {
    let memory = Memory::default();
    let every_other = EveryOther::default();
    let provider = application(every_other.clone(), &memory);
    let app = provider.tracer("the-application");
    let harness = Harness::with(Config {
        sampler: Some(Box::new(Sampler::ParentBased(Box::new(
            every_other.clone(),
        )))),
        ..keyed()
    })?;

    for _ in 0..100 {
        let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
        {
            let _inside = trace.attach();
            let mut inside = app.start("inside");
            inside.end();
        }
        trace.end();
    }
    harness.flush()?;
    assert_eq!(
        starting_with(&harness.spans()?, "invoke_agent ").len(),
        100,
        "every trace reaches Telmoni"
    );
    assert_eq!(memory.finished().len(), 50, "half, as outside traces");
    assert_eq!(
        every_other.calls.load(Ordering::SeqCst),
        100,
        "asked once per trace, for the root, and never for a span with a parent"
    );
    provider.shutdown()?;
    Ok(())
}

/// The application's span is not sampled, yet the trace linked to it is
/// decided by our own sampler and sent whole.
#[test]
fn a_trace_inside_a_span_the_application_does_not_sample_is_still_sent() -> Outcome {
    let memory = Memory::default();
    let provider = application(Sampler::AlwaysOff, &memory);
    let app = provider.tracer("the-application");
    let harness = Harness::new()?;

    let request = Context::current_with_span(app.start("request"));
    assert!(!request.span().span_context().is_sampled());
    let request_trace = request.span().span_context().trace_id().to_string();
    {
        let _current = request.clone().attach();
        let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
        let step = trace.step("s")?;
        step.end();
        trace.end();
    }
    request.span().end();
    harness.flush()?;

    let spans = harness.spans()?;
    let agent = only(&spans, "invoke_agent ")?;
    assert_ne!(agent.get("traceId"), Some(&json!(request_trace)));
    assert_eq!(
        agent.pointer("/links/0/traceId"),
        Some(&json!(request_trace)),
        "linked to the span it was opened under"
    );
    assert!(named(&spans, "s").is_ok());
    assert!(memory.finished().is_empty());
    provider.shutdown()?;
    Ok(())
}

/// A root sampler that tags its decision.
#[derive(Debug, Clone)]
struct Tagging;

impl ShouldSample for Tagging {
    fn should_sample(
        &self,
        _parent_context: Option<&Context>,
        _trace_id: TraceId,
        _name: &str,
        _span_kind: &SpanKind,
        _attributes: &[KeyValue],
        _links: &[Link],
    ) -> SamplingResult {
        SamplingResult {
            decision: SamplingDecision::RecordAndSample,
            attributes: vec![KeyValue::new("sampler.rule", "all")],
            trace_state: Default::default(),
        }
    }
}

/// A root sampler's decision is taken, never its attributes, as in every SDK.
#[test]
fn a_root_samplers_attributes_never_reach_the_trace() -> Outcome {
    let harness = Harness::with(Config {
        sampler: Some(Box::new(Tagging)),
        ..keyed()
    })?;
    harness.telmoni.trace("bot", TraceOptions::default())?.end();
    harness.flush()?;
    let spans = harness.spans()?;
    let trace = only(&spans, "invoke_agent ")?;
    assert!(attributes_of(trace)?.get("sampler.rule").is_none());
    Ok(())
}

#[test]
fn a_configured_sampler_decides_a_trace() -> Outcome {
    let memory = Memory::default();
    let provider = application(Sampler::AlwaysOn, &memory);
    let app = provider.tracer("the-application");
    let harness = Harness::with(Config {
        sampler: Some(Box::new(Sampler::AlwaysOff)),
        ..keyed()
    })?;

    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    {
        let _inside = trace.attach();
        let mut inside = app.start("inside");
        assert!(
            !inside.span_context().is_sampled(),
            "the trace is not sampled, so the application's span in it is not"
        );
        inside.end();
    }
    trace.end();
    harness.flush()?;
    assert_eq!(
        starting_with(&harness.spans()?, "invoke_agent ").len(),
        1,
        "sent regardless of the flag"
    );
    assert!(memory.finished().is_empty());
    provider.shutdown()?;
    Ok(())
}

#[test]
fn otel_variables_change_nothing_of_ours() -> Outcome {
    if is_child() {
        let harness = Harness::new()?;
        let trace = harness.telmoni.trace(
            "bot",
            TraceOptions {
                user_id: Some("c".to_owned()),
                session_id: Some("v".to_owned()),
                tags: tags(&[("a", "b")]),
                ..TraceOptions::default()
            },
        )?;
        trace.end();
        harness.flush()?;
        let spans = harness.spans()?;
        let attributes = attributes_of(only(&spans, "invoke_agent ")?)?;
        assert_eq!(attributes.get("telmoni.tag.a"), Some(&json!("b")));
        assert_eq!(attributes.get("user.id"), Some(&json!("c")));
        assert_eq!(attributes.get("gen_ai.conversation.id"), Some(&json!("v")));
        let bodies = harness.transport.bodies()?;
        let body = bodies.first().ok_or("a body")?;
        let resource_spans = field(body, "resourceSpans")?
            .as_array()
            .and_then(|all| all.first())
            .ok_or("a resourceSpans")?;
        let keys: HashSet<&str> = field(field(resource_spans, "resource")?, "attributes")?
            .as_array()
            .ok_or("resource attributes")?
            .iter()
            .filter_map(|attribute| attribute.get("key")?.as_str())
            .collect();
        assert_eq!(
            keys,
            HashSet::from([
                "telemetry.distro.name",
                "telemetry.distro.version",
                "telemetry.sdk.language",
                "telemetry.sdk.name",
                "telemetry.sdk.version",
            ])
        );
        return Ok(());
    }
    in_child(
        "otel_variables_change_nothing_of_ours",
        &[
            ("OTEL_SDK_DISABLED", "true"),
            ("OTEL_TRACES_SAMPLER", "always_off"),
            ("OTEL_RESOURCE_ATTRIBUTES", "leaked=yes"),
            ("OTEL_SERVICE_NAME", "leaked"),
            ("OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT", "1"),
            ("OTEL_BSP_MAX_QUEUE_SIZE", "1"),
        ],
        &[],
    )
}

#[test]
fn the_context_carries_the_trace_into_a_future() -> Outcome {
    let memory = Memory::default();
    let provider = application(Sampler::AlwaysOn, &memory);
    let app = provider.tracer("the-application");
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;

    let work = async {
        let mut inside = app.start("inside");
        let context = inside.span_context().clone();
        inside.end();
        context
    }
    .with_context(trace.context());
    let mut work = pin!(work);
    let Poll::Ready(inside) = work
        .as_mut()
        .poll(&mut TaskContext::from_waker(Waker::noop()))
    else {
        return Err("the future was not ready".into());
    };
    let trace_context = trace.context();
    let agent_span = trace_context.span().span_context().clone();
    assert_eq!(inside.trace_id(), agent_span.trace_id());
    let exported = memory.finished();
    let span = exported.first().ok_or("the application's span")?;
    assert_eq!(
        span.parent_span_id,
        agent_span.span_id(),
        "nested under the agent's span"
    );
    trace.end();
    harness.flush()?;
    provider.shutdown()?;
    Ok(())
}
