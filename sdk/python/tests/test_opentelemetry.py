"""The SDK beside an application's own OpenTelemetry: a trace is a trace of its
own, linked to the span the application had current, the application's spans
stay in their traces at their own sampling rate, a trace the application does
not sample is still sent, and its ``OTEL_*`` variables change nothing of ours."""

from __future__ import annotations

import os
from collections.abc import Sequence
from typing import Any
from unittest import mock

from opentelemetry import context, metrics, trace
from opentelemetry.metrics import Meter, NoOpMeterProvider
from opentelemetry.sdk.trace.export.in_memory_span_exporter import (
    InMemorySpanExporter,
)
from opentelemetry.sdk.trace.sampling import (
    ALWAYS_OFF,
    Decision,
    Sampler,
    SamplingResult,
)
from opentelemetry.trace import Span

from conftest import ENDPOINT, FakeTransport, SwitchableSampler, attributes_of
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


def our_agents(transport: FakeTransport) -> list[dict[str, object]]:
    return [s for s in transport.spans() if s["name"].startswith("invoke_agent ")]


def link_to(span: Span) -> dict[str, str]:
    """The link the agent's span carries to ``span``, as the export spells it."""
    span_context = span.get_span_context()
    return {
        "traceId": format(span_context.trace_id, "032x"),
        "spanId": format(span_context.span_id, "016x"),
    }


def test_a_trace_is_its_own_linked_to_the_applications_span(
    telmoni: Telmoni, transport: FakeTransport, app_exporter: InMemorySpanExporter
) -> None:
    app = trace.get_tracer("the-application")
    with (
        app.start_as_current_span("request") as request,
        telmoni.trace("bot") as opened,
        app.start_as_current_span("db.query"),
    ):
        pass
    assert telmoni.flush()
    (agent,) = our_agents(transport)
    assert agent["traceId"] == opened.trace_id
    assert agent["traceId"] != link_to(request)["traceId"], "a trace of its own"
    assert "parentSpanId" not in agent
    assert agent["links"] == [link_to(request)]
    app_spans: Sequence[Any] = app_exporter.get_finished_spans()
    assert sorted(s.name for s in app_spans) == ["db.query", "request"]
    query = next(s for s in app_spans if s.name == "db.query")
    assert format(query.parent.span_id, "016x") == agent["spanId"], (
        "nested under the agent's span"
    )
    assert format(query.context.trace_id, "032x") == agent["traceId"]
    assert not any(s["name"] in ("request", "db.query") for s in transport.spans())


def test_the_application_keeps_its_own_sampling_rate_inside_traces(
    telmoni: Telmoni,
    transport: FakeTransport,
    app_exporter: InMemorySpanExporter,
    app_sampler: SwitchableSampler,
) -> None:
    app_sampler.target = EveryOther()
    app = trace.get_tracer("the-application")
    for _ in range(100):
        with telmoni.trace("bot"), app.start_as_current_span("inside"):
            pass
    assert telmoni.flush()
    assert len(our_agents(transport)) == 100, "every trace reaches Telmoni"
    assert len(app_exporter.get_finished_spans()) == 50, "half, as outside traces"


def test_a_trace_opened_in_a_span_the_application_does_not_sample_is_still_sent(
    telmoni: Telmoni,
    transport: FakeTransport,
    app_exporter: InMemorySpanExporter,
    app_sampler: SwitchableSampler,
) -> None:
    app_sampler.target = ALWAYS_OFF
    app = trace.get_tracer("the-application")
    with app.start_as_current_span("request") as request:
        assert not request.get_span_context().trace_flags.sampled
        with telmoni.trace("bot"), telmoni.step("s"):
            pass
    assert telmoni.flush()
    (agent,) = our_agents(transport)
    assert agent["traceId"] != link_to(request)["traceId"]
    assert agent["links"] == [link_to(request)], "linked to the span all the same"
    assert any(s["name"] == "s" for s in transport.spans())
    assert app_exporter.get_finished_spans() == ()


def test_a_configured_sampler_decides_a_trace(transport: FakeTransport) -> None:
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k", sampler=ALWAYS_OFF), transport=transport
    )
    try:
        app = trace.get_tracer("the-application")
        with client.trace("bot"), app.start_as_current_span("inside") as inside:
            assert not inside.get_span_context().trace_flags.sampled
        assert client.flush()
    finally:
        client.shutdown()
    assert len(our_agents(transport)) == 1, "sent regardless of the flag"


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
        client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        with client.trace("bot", user_id="c", session_id="v", tags={"a": "b"}):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    (agent,) = our_agents(transport)
    attributes = attributes_of(agent)
    assert attributes["telmoni.tag.a"] == "b" and attributes["user.id"] == "c"
    resource = transport.bodies()[0]["resourceSpans"][0]["resource"]["attributes"]
    assert {a["key"] for a in resource} == {
        "telemetry.distro.name",
        "telemetry.distro.version",
        "telemetry.sdk.language",
        "telemetry.sdk.name",
        "telemetry.sdk.version",
    }


def test_our_span_metrics_never_reach_the_applications_meter(
    transport: FakeTransport,
) -> None:
    """Turned on by an ``OTEL_*`` variable, they go to a meter of our own."""
    asked: list[str] = []

    class Counting(NoOpMeterProvider):
        def get_meter(self, name: str, *args: Any, **kwargs: Any) -> Meter:
            asked.append(name)
            return super().get_meter(name, *args, **kwargs)

    env = {"OTEL_PYTHON_SDK_INTERNAL_METRICS_ENABLED": "true"}
    with (
        mock.patch.dict(os.environ, env),
        mock.patch.object(metrics, "get_meter_provider", Counting),
    ):
        client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
        try:
            with client.trace("bot"):
                pass
            assert client.flush()
        finally:
            client.shutdown()
    assert asked == []


def test_the_trace_is_found_from_the_context_in_another_thread(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    import threading

    with telmoni.trace("bot") as opened:
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
    assert step["traceId"] == opened.trace_id
