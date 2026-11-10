from __future__ import annotations

import enum
import logging
from collections.abc import Iterator
from concurrent.futures import ThreadPoolExecutor

import pytest

from conftest import FakeTransport, attributes_of
from telmoni import Telmoni, Trace


def by_name(transport: FakeTransport) -> dict[str, dict[str, object]]:
    return {str(s["name"]): s for s in transport.spans()}


def test_a_step_nests_under_its_agents_span_and_carries_its_ids(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with (
        telmoni.trace("bot", user_id="c", session_id="v") as trace,
        telmoni.step("plan"),
        telmoni.step("plan.inner"),
    ):
        pass
    assert telmoni.flush()
    spans = by_name(transport)
    plan, inner, agent = spans["plan"], spans["plan.inner"], spans["invoke_agent bot"]
    assert plan["parentSpanId"] == agent["spanId"]
    assert inner["parentSpanId"] == plan["spanId"]
    assert plan["traceId"] == agent["traceId"] == inner["traceId"] == trace.trace_id
    assert attributes_of(plan) == {
        "telmoni.agent": "bot",
        "user.id": "c",
        "gen_ai.conversation.id": "v",
    }
    assert plan["kind"] == 1 and "status" not in plan


def test_a_step_opened_on_the_trace_handle_is_the_same(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.trace("bot") as trace, trace.step("explicit"):
        pass
    assert telmoni.flush()
    step = by_name(transport)["explicit"]
    assert step["traceId"] == trace.trace_id


def test_a_step_or_call_from_a_worker_thread_goes_under_the_agents_span(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    def work(trace: Trace) -> None:
        # A pool's thread starts with an empty context: neither the agent's
        # span nor the trace is current in it.
        with trace.step("think"), telmoni.tool_call("search"):
            pass
        with trace.model_call("m"):
            pass

    with telmoni.trace("bot") as trace, ThreadPoolExecutor(max_workers=1) as pool:
        pool.submit(work, trace).result()
    assert telmoni.flush()
    spans = by_name(transport)
    agent = spans["invoke_agent bot"]
    for name in ("think", "chat m"):
        assert spans[name]["parentSpanId"] == agent["spanId"], name
        assert spans[name]["traceId"] == agent["traceId"], name
    # The client-level call inside the step found the trace through it.
    search = spans["execute_tool search"]
    assert search["parentSpanId"] == spans["think"]["spanId"]
    assert search["traceId"] == trace.trace_id
    assert not [r for r in caplog.records if r.levelno >= logging.WARNING]


def test_a_failing_step_records_the_error_class_and_reraises(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    class PlanningFailed(RuntimeError):
        pass

    with telmoni.trace("bot"):
        try:
            with telmoni.step("plan"):
                raise PlanningFailed("secret details")
        except PlanningFailed:
            pass
    assert telmoni.flush()
    spans = by_name(transport)
    assert spans["plan"]["status"] == {"code": 2}
    assert attributes_of(spans["plan"])["error.type"] == "PlanningFailed"
    assert "status" not in spans["invoke_agent bot"], "the trace itself went on"
    assert "secret details" not in str(transport.requests)


def test_the_step_decorator(telmoni: Telmoni, transport: FakeTransport) -> None:
    @telmoni.step("plan")
    def plan(n: int) -> int:
        return n + 1

    with telmoni.trace("bot"):
        assert plan(1) == 2
    assert telmoni.flush()
    assert "plan" in by_name(transport)


def test_the_step_decorator_on_a_generator_spans_its_iteration(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    @telmoni.step("plan")
    def plan() -> Iterator[int]:
        yield 1
        with telmoni.tool_call("search"):
            pass
        yield 2

    with telmoni.trace("bot"):
        assert list(plan()) == [1, 2]
    assert telmoni.flush()
    spans = by_name(transport)
    assert spans["execute_tool search"]["parentSpanId"] == spans["plan"]["spanId"]


class Kind(enum.Enum):
    PLAN = "plan"


def test_a_name_of_no_string_goes_as_its_text_and_keeps_the_batch(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Sent as given, it would fail the export's JSON and lose every trace's spans."""
    with telmoni.trace("other"), telmoni.step("fine"):
        pass
    with telmoni.trace(Kind.PLAN) as trace:  # type: ignore[arg-type]
        with trace.step(Kind.PLAN):  # type: ignore[arg-type]
            pass
        with trace.model_call(5):  # type: ignore[arg-type]
            pass
        with trace.tool_call(Kind.PLAN):  # type: ignore[arg-type]
            pass
    assert telmoni.flush()
    names = sorted(span["name"] for span in transport.spans())
    assert names == [
        "Kind.PLAN",
        "chat 5",
        "execute_tool Kind.PLAN",
        "fine",
        "invoke_agent Kind.PLAN",
        "invoke_agent other",
    ]
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert len(warnings) == 4 and all("is sent as its text" in w for w in warnings)


def test_a_step_named_nothing_or_past_ingests_cap_keeps_its_span(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Ingest refuses a span named nothing, or past 145 characters."""
    with telmoni.trace("bot") as trace:
        for name in ("", "", "n" * 146, "n" * 146, "n" * 145):
            with trace.step(name):
                pass
    assert telmoni.flush()
    steps = sorted(
        s["name"] for s in transport.spans() if s["name"] != "invoke_agent bot"
    )
    assert steps == ["n" * 145] * 3 + ["step"] * 2
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warnings == [
        "telmoni: a step needs its name; one without is sent named step",
        (
            "telmoni: a step's name must be up to 145 characters; "
            "it is sent cut to its first 145"
        ),
    ]


def test_a_name_with_a_lone_surrogate_goes_with_the_replacement_character(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    """Escaped into the JSON, ingest would refuse the request and the batch."""
    for name in ("a\ud800b", "c\udc80d", "e\U0001f600f"):
        with telmoni.trace("bot"), telmoni.step(name):
            pass
    assert telmoni.flush()
    names = {span["name"] for span in transport.spans()}
    assert {"a\ufffdb", "c\ufffdd", "e\U0001f600f"} <= names
