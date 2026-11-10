//! The processor: a bounded queue whose drops are counted and warned once, a
//! batch that goes when it fills, and a shutdown that drains.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::{FakeTransport, Harness, Outcome, Sleeps, Warnings, client, hooks, keyed, name_of};
use telmoni::transport::{Answer, Method, Transport, TransportError};
use telmoni::{Batching, Telmoni, TraceOptions};

#[test]
fn the_queue_is_bounded_and_drops_are_counted_once() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    // A tiny queue, and a worker that will not wake until flushed.
    hooks.batching = Batching {
        max_queue_size: 2,
        max_batch_size: 100,
        schedule_delay: Duration::from_secs(60),
    };
    let telmoni = Telmoni::with_hooks(keyed(), hooks);

    let trace = telmoni.trace("bot", TraceOptions::default())?;
    // The trace's opening goes at once; the queue is filled after it.
    assert!(transport.wait_for(1), "the trace's opening");
    for n in 0..5 {
        let step = trace.step(&format!("s{n}"))?;
        step.end();
    }
    trace.end();
    assert!(telmoni.flush());
    telmoni.shutdown();

    let mut names: Vec<String> = transport
        .spans()?
        .iter()
        .map(|span| name_of(span).to_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["s0", "s1"]);
    let warned = warnings.all();
    assert_eq!(warned.len(), 1, "{warned:?}");
    let warning = warned.first().ok_or("a warning")?;
    assert!(warning.contains("queue was full"), "{warning}");
    assert!(
        warning.contains("4 spans"),
        "three steps and the agent's span itself: {warning}"
    );
    assert_eq!(warnings.kinds(), ["queue_full"]);
    Ok(())
}

/// A delay of nothing sends at once, and leaves an idle worker waiting to be
/// told rather than spinning on a timer of nothing.
#[test]
fn a_delay_of_nothing_sends_at_once() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    hooks.batching = Batching {
        schedule_delay: Duration::ZERO,
        ..Batching::default()
    };
    let telmoni = Telmoni::with_hooks(keyed(), hooks);
    // Each trace's opening goes first, at once, whatever the delay.
    let trace = telmoni.trace("bot", TraceOptions::default())?;
    assert!(transport.wait_for(1), "the trace's opening");
    trace.end();
    assert!(
        transport.wait_for(2),
        "the trace's span was not sent at once"
    );
    let trace = telmoni.trace("bot", TraceOptions::default())?;
    assert!(transport.wait_for(3), "the trace's opening");
    trace.end();
    assert!(transport.wait_for(4), "an idle worker missed the next span");
    telmoni.shutdown();
    Ok(())
}

#[test]
fn shutdown_drains_what_is_queued() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    trace.end();
    harness.telmoni.shutdown();
    assert_eq!(
        harness.spans()?.iter().map(name_of).collect::<Vec<_>>(),
        ["invoke_agent bot"]
    );
    harness.telmoni.shutdown();
    Ok(())
}

#[test]
fn dropping_the_client_drains_what_is_queued() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    {
        let telmoni = client(keyed(), &transport, &warnings, &sleeps);
        let trace = telmoni.trace("bot", TraceOptions::default())?;
        trace.end();
    }
    assert_eq!(
        transport.spans()?.iter().map(name_of).collect::<Vec<_>>(),
        ["invoke_agent bot"]
    );
    Ok(())
}

#[test]
fn a_batch_fills_and_goes_without_a_flush() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    hooks.batching = Batching {
        max_batch_size: 3,
        schedule_delay: Duration::from_secs(60),
        ..Batching::default()
    };
    let telmoni = Telmoni::with_hooks(keyed(), hooks);

    let trace = telmoni.trace("bot", TraceOptions::default())?;
    assert!(transport.wait_for(1), "the trace's opening");
    for n in 0..3 {
        let step = trace.step(&format!("s{n}"))?;
        step.end();
    }
    assert!(transport.wait_for(2), "three steps fill a batch of three");
    trace.end();
    telmoni.shutdown();
    assert!(warnings.all().is_empty());
    Ok(())
}

/// ⚠ A hook of the application's that panics — here `warn`, on the first
/// warning — kills no worker: the warning is lost, the batch beside it and the
/// next one still go, and a flush and a shutdown still return.
#[test]
fn a_hook_that_panics_does_not_stop_the_exporter() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    hooks.warn = Box::new(|text, _| std::panic::panic_any(text.to_owned()));
    hooks.batching = Batching {
        max_queue_size: 1,
        max_batch_size: 100,
        schedule_delay: Duration::from_secs(60),
    };
    let telmoni = Telmoni::with_hooks(keyed(), hooks);

    // The steps overflow the queue before the worker wakes, so the batch the
    // flush takes carries a drop count and the hook panics on its warning.
    let trace = telmoni.trace("bot", TraceOptions::default())?;
    assert!(
        transport.wait_for(1),
        "the trace's opening, out of the queue"
    );
    for n in 0..3 {
        let step = trace.step(&format!("s{n}"))?;
        step.end();
    }
    trace.end();
    assert!(
        telmoni.flush(),
        "the flush waited on a worker that was gone"
    );

    // The worker is alive: a trace after the panic is sent.
    let sent = transport.count();
    let again = telmoni.trace("bot", TraceOptions::default())?;
    assert!(transport.wait_for(sent + 1), "the trace's opening");
    again.end();
    assert!(telmoni.flush());
    telmoni.shutdown();

    let spans = transport.spans()?;
    assert!(
        spans.iter().any(|span| name_of(span) == "s0"),
        "the batch beside the warning that panicked was not sent"
    );
    let agents = spans
        .iter()
        .filter(|span| name_of(span) == "invoke_agent bot")
        .count();
    assert_eq!(agents, 1, "the trace after the panic was not sent");
    Ok(())
}

/// ⚠ A transport that panics loses that batch alone: a flush returns and the next trace goes.
#[test]
fn a_transport_that_panics_does_not_stop_the_exporter() -> Outcome {
    /// Panics on its first export once armed, then hands each to the fake.
    struct PanicsOnce(Arc<AtomicBool>, FakeTransport);

    impl Transport for PanicsOnce {
        fn send(
            &self,
            method: Method,
            url: &str,
            headers: &[(String, String)],
            body: &[u8],
        ) -> Result<Answer, TransportError> {
            if self.0.swap(false, Ordering::SeqCst) {
                std::panic::panic_any("a transport of the application's");
            }
            self.1.send(method, url, headers, body)
        }
    }

    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    let armed = Arc::new(AtomicBool::new(false));
    hooks.transport = Box::new(PanicsOnce(Arc::clone(&armed), transport.clone()));
    let telmoni = Telmoni::with_hooks(keyed(), hooks);

    // Armed past the trace's opening, so the batch lost is the trace's own.
    let lost = telmoni.trace("lost", TraceOptions::default())?;
    assert!(transport.wait_for(1), "the trace's opening");
    armed.store(true, Ordering::SeqCst);
    lost.end();
    assert!(
        telmoni.flush(),
        "the flush waited on a worker that was gone"
    );
    telmoni.trace("sent", TraceOptions::default())?.end();
    assert!(telmoni.flush());
    telmoni.shutdown();
    let spans = transport.spans()?;
    let names: Vec<_> = spans.iter().map(name_of).collect();
    assert_eq!(names, ["invoke_agent sent"]);
    assert_eq!(
        warnings.all(),
        ["telmoni: a hook panicked during an export; the batch was dropped"]
    );
    assert_eq!(warnings.kinds(), ["export_error"]);
    Ok(())
}
