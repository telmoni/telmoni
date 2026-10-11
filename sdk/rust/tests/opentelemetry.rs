//! The SDK beside an application's own OpenTelemetry: its spans stay in their
//! traces at their own sampling rate, a run inside a trace it does not sample
//! is still sent, and its `OTEL_*` variables change nothing of ours.

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
use telmoni_sdk::{Config, RunOptions};

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

#[test]
fn a_run_joins_the_current_trace_and_the_application_keeps_its_span() -> Outcome {
    let memory = Memory::default();
    let provider = application(Sampler::AlwaysOn, &memory);
    let app = provider.tracer("the-application");
    let harness = Harness::new()?;

    let request = Context::current_with_span(app.start("request"));
    let request_trace = request.span().span_context().trace_id().to_string();
    let request_span = request.span().span_context().span_id().to_string();
    {
        let _current = request.clone().attach();
        let run = harness.telmoni.run("bot", RunOptions::default())?;
        {
            let _inside = run.attach();
            let mut query = app.start("db.query");
            query.end();
        }
        run.end();
    }
    request.span().end();
    harness.flush()?;

    let spans = harness.spans()?;
    let run = only(&spans, "invoke_agent ")?;
    assert_eq!(run.get("traceId"), Some(&json!(request_trace)));
    assert_eq!(run.get("parentSpanId"), Some(&json!(request_span)));
    let app_spans = memory.finished();
    let mut names: Vec<&str> = app_spans.iter().map(|span| span.name.as_ref()).collect();
    names.sort_unstable();
    assert_eq!(names, ["db.query", "request"]);
    let query = app_spans
        .iter()
        .find(|span| span.name == "db.query")
        .ok_or("db.query")?;
    assert_eq!(
        run.get("spanId"),
        Some(&json!(query.parent_span_id.to_string())),
        "nested under the run"
    );
    assert_eq!(
        run.get("traceId"),
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
fn the_application_keeps_its_own_sampling_rate_inside_runs() -> Outcome {
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
        let run = harness.telmoni.run("bot", RunOptions::default())?;
        {
            let _inside = run.attach();
            let mut inside = app.start("inside");
            inside.end();
        }
        run.end();
    }
    harness.flush()?;
    assert_eq!(
        starting_with(&harness.spans()?, "invoke_agent ").len(),
        100,
        "every run reaches Telmoni"
    );
    assert_eq!(memory.finished().len(), 50, "half, as outside runs");
    assert_eq!(
        every_other.calls.load(Ordering::SeqCst),
        100,
        "asked once per run, for the root, and never for a span with a parent"
    );
    provider.shutdown()?;
    Ok(())
}

#[test]
fn a_run_inside_a_trace_the_application_does_not_sample_is_still_sent() -> Outcome {
    let memory = Memory::default();
    let provider = application(Sampler::AlwaysOff, &memory);
    let app = provider.tracer("the-application");
    let harness = Harness::new()?;

    let request = Context::current_with_span(app.start("request"));
    assert!(!request.span().span_context().is_sampled());
    let request_trace = request.span().span_context().trace_id().to_string();
    {
        let _current = request.clone().attach();
        let run = harness.telmoni.run("bot", RunOptions::default())?;
        let step = run.step("s");
        step.end();
        run.end();
    }
    request.span().end();
    harness.flush()?;

    let spans = harness.spans()?;
    let run = only(&spans, "invoke_agent ")?;
    assert_eq!(run.get("traceId"), Some(&json!(request_trace)));
    assert!(named(&spans, "s").is_ok());
    assert!(memory.finished().is_empty());
    provider.shutdown()?;
    Ok(())
}

#[test]
fn a_configured_sampler_decides_a_root_run() -> Outcome {
    let memory = Memory::default();
    let provider = application(Sampler::AlwaysOn, &memory);
    let app = provider.tracer("the-application");
    let harness = Harness::with(Config {
        sampler: Some(Box::new(Sampler::AlwaysOff)),
        ..keyed()
    })?;

    let run = harness.telmoni.run("bot", RunOptions::default())?;
    {
        let _inside = run.attach();
        let mut inside = app.start("inside");
        assert!(
            !inside.span_context().is_sampled(),
            "the run's trace is not sampled, so the application's span is not"
        );
        inside.end();
    }
    run.end();
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
        let run = harness.telmoni.run(
            "bot",
            RunOptions {
                user_id: Some("c".to_owned()),
                conversation_id: Some("v".to_owned()),
                tags: tags(&[("a", "b")]),
                ..RunOptions::default()
            },
        )?;
        run.end();
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
fn the_context_carries_the_run_into_a_future() -> Outcome {
    let memory = Memory::default();
    let provider = application(Sampler::AlwaysOn, &memory);
    let app = provider.tracer("the-application");
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;

    let work = async {
        let mut inside = app.start("inside");
        let context = inside.span_context().clone();
        inside.end();
        context
    }
    .with_context(run.context());
    let mut work = pin!(work);
    let Poll::Ready(inside) = work
        .as_mut()
        .poll(&mut TaskContext::from_waker(Waker::noop()))
    else {
        return Err("the future was not ready".into());
    };
    let run_context = run.context();
    let run_span = run_context.span().span_context().clone();
    assert_eq!(inside.trace_id(), run_span.trace_id());
    let exported = memory.finished();
    let span = exported.first().ok_or("the application's span")?;
    assert_eq!(
        span.parent_span_id,
        run_span.span_id(),
        "nested under the run"
    );
    run.end();
    harness.flush()?;
    provider.shutdown()?;
    Ok(())
}
