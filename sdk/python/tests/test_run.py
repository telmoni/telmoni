from __future__ import annotations

import asyncio
import logging
import re
import threading
from unittest import mock

import pytest

from conftest import FakeTransport, attributes_of
from telmoni import Bounds, Config, Run, Telmoni, current_run

UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$")


def run_span(transport: FakeTransport) -> dict[str, object]:
    runs = [s for s in transport.spans() if s["name"].startswith("invoke_agent ")]
    assert len(runs) == 1
    return runs[0]


def test_a_run_is_a_span_with_the_contract_attributes(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.run(
        "support-bot",
        user_id="user_1",
        session_id="conv_1",
        tags={"env": "test", "team": "a"},
        bounds=Bounds(stall_after_s=30, max_steps=5, max_cost_usd=0.5),
    ) as run:
        assert UUID.match(run.run_id)
        assert run.agent == "support-bot"
        assert current_run() is run
    assert current_run() is None
    assert telmoni.flush()
    span = run_span(transport)
    assert span["kind"] == 1
    assert "parentSpanId" not in span
    assert "status" not in span
    assert attributes_of(span) == {
        "gen_ai.operation.name": "invoke_agent",
        "gen_ai.agent.name": "support-bot",
        "telmoni.run_id": run.run_id,
        "telmoni.agent": "support-bot",
        "user.id": "user_1",
        "gen_ai.conversation.id": "conv_1",
        "telmoni.bound.stall_after_s": 30,
        "telmoni.bound.max_steps": 5,
        "telmoni.bound.max_cost_usd": 0.5,
        "telmoni.sample_rate": 1.0,
        "telmoni.tag.env": "test",
        "telmoni.tag.team": "a",
    }


def test_the_start_is_sent_at_once_with_the_bounds(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.run("bot", bounds=Bounds(max_steps=3)) as run:
        assert transport.wait_for(1)
        spans = transport.spans()
        assert [s["name"] for s in spans] == ["telmoni.run.start"]
        start = spans[0]
        assert start["startTimeUnixNano"] == start["endTimeUnixNano"]
        assert attributes_of(start) == {
            "telmoni.run_id": run.run_id,
            "telmoni.agent": "bot",
            "telmoni.bound.max_steps": 3,
            "telmoni.sample_rate": 1.0,
        }
    assert telmoni.flush()
    assert run_span(transport)["spanId"] == transport.spans()[0]["parentSpanId"]


def test_an_exception_fails_the_run_with_its_class_alone(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with pytest.raises(TimeoutError), telmoni.run("bot"):
        raise TimeoutError("the message, which is content and never sent")
    assert telmoni.flush()
    span = run_span(transport)
    assert span["status"] == {"code": 2}
    assert attributes_of(span)["error.type"] == "TimeoutError"
    assert "never sent" not in str(transport.requests)


def test_fail_and_cancel_mark_the_run(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.run("bot") as run:
        run.fail("RateLimit")
    with telmoni.run("bot") as run:
        run.cancel()
    assert telmoni.flush()
    runs = sorted(
        (s for s in transport.spans() if s["name"].startswith("invoke_agent ")),
        key=lambda s: "error.type" not in attributes_of(s),
    )
    assert attributes_of(runs[0])["error.type"] == "RateLimit" and runs[0][
        "status"
    ] == {"code": 2}
    assert (
        attributes_of(runs[1])["telmoni.cancelled"] is True and "status" not in runs[1]
    )


def test_a_keyboard_interrupt_is_a_cancellation(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with pytest.raises(KeyboardInterrupt), telmoni.run("bot"):
        raise KeyboardInterrupt
    assert telmoni.flush()
    span = run_span(transport)
    assert attributes_of(span)["telmoni.cancelled"] is True
    assert "status" not in span


def test_the_decorator_opens_a_run_per_call(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    seen: list[str] = []

    @telmoni.run("bot")
    def handle(question: str) -> str:
        run = current_run()
        assert run is not None
        seen.append(run.run_id)
        return question.upper()

    assert handle("a") == "A"
    assert handle("b") == "B"
    assert telmoni.flush()
    runs = [s for s in transport.spans() if s["name"] == "invoke_agent bot"]
    assert len(runs) == 2 and len(set(seen)) == 2
    assert handle.__name__ == "handle"


def test_the_decorator_and_context_manager_work_in_asyncio(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    @telmoni.run("bot")
    async def handle() -> str:
        run = current_run()
        assert run is not None
        async with telmoni.step("think"):
            await asyncio.sleep(0)
        return run.run_id

    async def main() -> list[str]:
        ids = [await handle()]
        async with telmoni.run("bot") as run:
            ids.append(run.run_id)
        return ids

    ids = asyncio.run(main())
    assert telmoni.flush()
    spans = transport.spans()
    assert len([s for s in spans if s["name"] == "invoke_agent bot"]) == 2
    step = next(s for s in spans if s["name"] == "think")
    assert attributes_of(step)["telmoni.run_id"] == ids[0]


def test_nothing_is_sent_outside_a_run(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    with telmoni.step("alone") as step:
        step.fail("x")
    with telmoni.model_call("m") as call:
        call.answered(response_model="m2")
    with telmoni.tool_call("t") as tool:
        tool.returned({"a": 1})
    assert telmoni.flush()
    assert transport.requests == []
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert len(warnings) == 1 and "outside a run" in warnings[0]


def test_the_sample_rate_drops_whole_runs_in_the_processor(
    transport: FakeTransport,
) -> None:
    client = Telmoni(Config(api_key="k", sample_rate=0.25), transport=transport)
    try:
        with (
            mock.patch("telmoni.runs.random.random", return_value=0.9),
            client.run("bot"),
            client.step("s"),
        ):
            pass
        with (
            mock.patch("telmoni.runs.random.random", return_value=0.1),
            client.run("bot") as kept,
        ):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    spans = transport.spans()
    assert {attributes_of(s)["telmoni.run_id"] for s in spans} == {kept.run_id}
    assert {s["name"] for s in spans} == {"telmoni.run.start", "invoke_agent bot"}
    assert attributes_of(spans[0])["telmoni.sample_rate"] == 0.25


def test_a_span_ending_after_its_sampled_out_run_is_dropped_too(
    transport: FakeTransport,
) -> None:
    client = Telmoni(Config(api_key="k", sample_rate=0.25), transport=transport)
    opened, released = threading.Event(), threading.Event()

    def late(run: Run) -> None:
        with run.step("late"):
            opened.set()
            released.wait(5)

    try:
        with (
            mock.patch("telmoni.runs.random.random", return_value=0.9),
            client.run("bot") as dropped,
        ):
            worker = threading.Thread(target=late, args=(dropped,))
            worker.start()
            assert opened.wait(5)
        released.set()
        worker.join(5)
        with (
            mock.patch("telmoni.runs.random.random", return_value=0.1),
            client.run("bot") as kept,
        ):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    spans = transport.spans()
    assert {attributes_of(s)["telmoni.run_id"] for s in spans} == {kept.run_id}
    assert current_run() is None, "the test leaves no run current"


def test_a_run_needs_an_agent_and_a_valid_rate(telmoni: Telmoni) -> None:
    with pytest.raises(ValueError), telmoni.run(" "):
        pass
    with pytest.raises(ValueError), telmoni.run("bot", sample_rate=0):
        pass


def test_a_bound_json_cannot_carry_is_left_off_with_one_warning(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    for bounds in (
        Bounds(max_cost_usd=float("inf"), max_steps=10),
        Bounds(max_cost_usd=float("nan")),
        Bounds(stall_after_s=2**63, max_steps="7"),  # type: ignore[arg-type]
    ):
        with telmoni.run("bot", bounds=bounds):
            pass
    assert telmoni.flush()
    runs = [s for s in transport.spans() if s["name"] == "invoke_agent bot"]
    assert len(runs) == 3, "every run sent, and the batch with them"
    sent = [
        {k: v for k, v in attributes_of(s).items() if k.startswith("telmoni.bound.")}
        for s in runs
    ]
    assert sent == [
        {"telmoni.bound.max_steps": 10},
        {},
        {"telmoni.bound.max_steps": 7},
    ]
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert len(warnings) == 2, warnings
    assert all("must be a finite number within 64 bits" in w for w in warnings)
