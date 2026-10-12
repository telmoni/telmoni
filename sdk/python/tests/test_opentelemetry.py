"""The SDK beside an application's own OpenTelemetry: its spans stay in their
traces at their own sampling rate, a run inside a trace it does not sample is
still sent, and its ``OTEL_*`` variables change nothing of ours."""

from __future__ import annotations

import os
from collections.abc import Sequence
from typing import Any
from unittest import mock

from opentelemetry import context, trace
from opentelemetry.sdk.trace.export.in_memory_span_exporter import (
    InMemorySpanExporter,
)
from opentelemetry.sdk.trace.sampling import (
    ALWAYS_OFF,
    Decision,
    Sampler,
    SamplingResult,
)

from conftest import FakeTransport, SwitchableSampler, attributes_of
from telmoni import Config, Telmoni


class EveryOther(Sampler):
    """Samples every second root span: a rate of one half, deterministic."""

    def __init__(self) -> None:
        self.calls = 0

    def should_sample(self, *args: Any, **kwargs: Any) -> SamplingResult:
        self.calls += 1
        decision = Decision.RECORD_AND_SAMPLE if self.calls % 2 == 0 else Decision.DROP
        return SamplingResult(decision)

    def get_description(self) -> str:
        return "EveryOther"


def our_runs(transport: FakeTransport) -> list[dict[str, object]]:
    return [s for s in transport.spans() if s["name"].startswith("invoke_agent ")]


def test_a_run_joins_the_current_trace_and_the_application_keeps_its_span(
    telmoni: Telmoni, transport: FakeTransport, app_exporter: InMemorySpanExporter
) -> None:
    app = trace.get_tracer("the-application")
    with (
        app.start_as_current_span("request") as request,
        telmoni.run("bot"),
        app.start_as_current_span("db.query"),
    ):
        pass
    assert telmoni.flush()
    (run,) = our_runs(transport)
    request_context = request.get_span_context()
    assert run["traceId"] == format(request_context.trace_id, "032x")
    assert run["parentSpanId"] == format(request_context.span_id, "016x")
    app_spans: Sequence[Any] = app_exporter.get_finished_spans()
    assert sorted(s.name for s in app_spans) == ["db.query", "request"]
    query = next(s for s in app_spans if s.name == "db.query")
    assert format(query.parent.span_id, "016x") == run["spanId"], "nested under the run"
    assert format(query.context.trace_id, "032x") == run["traceId"]
    assert not any(s["name"] in ("request", "db.query") for s in transport.spans())


def test_the_application_keeps_its_own_sampling_rate_inside_runs(
    telmoni: Telmoni,
    transport: FakeTransport,
    app_exporter: InMemorySpanExporter,
    app_sampler: SwitchableSampler,
) -> None:
    app_sampler.target = EveryOther()
    app = trace.get_tracer("the-application")
    for _ in range(100):
        with telmoni.run("bot"), app.start_as_current_span("inside"):
            pass
    assert telmoni.flush()
    assert len(our_runs(transport)) == 100, "every run reaches Telmoni"
    assert len(app_exporter.get_finished_spans()) == 50, "half, as outside runs"


def test_a_run_inside_a_trace_the_application_does_not_sample_is_still_sent(
    telmoni: Telmoni,
    transport: FakeTransport,
    app_exporter: InMemorySpanExporter,
    app_sampler: SwitchableSampler,
) -> None:
    app_sampler.target = ALWAYS_OFF
    app = trace.get_tracer("the-application")
    with app.start_as_current_span("request") as request:
        assert not request.get_span_context().trace_flags.sampled
        with telmoni.run("bot"), telmoni.step("s"):
            pass
    assert telmoni.flush()
    (run,) = our_runs(transport)
    assert run["traceId"] == format(request.get_span_context().trace_id, "032x")
    assert any(s["name"] == "s" for s in transport.spans())
    assert app_exporter.get_finished_spans() == ()


def test_a_configured_sampler_decides_a_root_run(transport: FakeTransport) -> None:
    client = Telmoni(Config(api_key="k", sampler=ALWAYS_OFF), transport=transport)
    try:
        app = trace.get_tracer("the-application")
        with client.run("bot"), app.start_as_current_span("inside") as inside:
            assert not inside.get_span_context().trace_flags.sampled
        assert client.flush()
    finally:
        client.shutdown()
    assert len(our_runs(transport)) == 1, "sent regardless of the flag"


def test_otel_variables_change_nothing_of_ours(transport: FakeTransport) -> None:
    env = {
        "OTEL_SDK_DISABLED": "true",
        "OTEL_TRACES_SAMPLER": "always_off",
        "OTEL_RESOURCE_ATTRIBUTES": "leaked=yes",
        "OTEL_SERVICE_NAME": "leaked",
        "OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT": "1",
        "OTEL_BSP_MAX_QUEUE_SIZE": "1",
    }
    with mock.patch.dict(os.environ, env):
        client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        with client.run("bot", user_id="c", session_id="v", tags={"a": "b"}):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    (run,) = our_runs(transport)
    attributes = attributes_of(run)
    assert attributes["telmoni.tag.a"] == "b" and attributes["user.id"] == "c"
    resource = transport.bodies()[0]["resourceSpans"][0]["resource"]["attributes"]
    assert {a["key"] for a in resource} == {
        "telemetry.distro.name",
        "telemetry.distro.version",
        "telemetry.sdk.language",
        "telemetry.sdk.name",
        "telemetry.sdk.version",
    }


def test_the_run_is_found_from_the_context_in_another_thread(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    import threading

    with telmoni.run("bot") as run:
        ctx = context.get_current()

        def worker() -> None:
            token = context.attach(ctx)
            try:
                with telmoni.step("threaded"):
                    pass
            finally:
                context.detach(token)

        thread = threading.Thread(target=worker)
        thread.start()
        thread.join()
    assert telmoni.flush()
    step = next(s for s in transport.spans() if s["name"] == "threaded")
    assert attributes_of(step)["telmoni.run_id"] == run.run_id
