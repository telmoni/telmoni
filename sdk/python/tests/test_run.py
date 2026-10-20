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


def test_a_fail_of_an_empty_type_fails_the_run_with_none(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    """Ingest refuses a span over an empty ``error.type``: here, the run."""
    with telmoni.run("bot") as run:
        run.fail("")
    assert telmoni.flush()
    (failed,) = (s for s in transport.spans() if s["name"] == "invoke_agent bot")
    assert "error.type" not in attributes_of(failed)
    assert failed["status"] == {"code": 2}


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


def test_a_bound_ingest_would_refuse_the_run_for_is_left_off_with_one_warning(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    for bounds in (
        Bounds(max_cost_usd=float("inf"), max_steps=10),
        Bounds(max_cost_usd=float("nan")),
        Bounds(stall_after_s=2**63, max_steps="7"),  # type: ignore[arg-type]
        Bounds(stall_after_s=2**32, max_steps=0, max_cost_usd=0.0),
        Bounds(stall_after_s=2**32 - 1, max_steps=-1, max_cost_usd=-1.0),
        Bounds(max_steps=True),
        Bounds(max_cost_usd=True),
    ):
        with telmoni.run("bot", bounds=bounds):
            pass
    assert telmoni.flush()
    runs = [s for s in transport.spans() if s["name"] == "invoke_agent bot"]
    assert len(runs) == 7, "every run sent, and the batch with them"
    sent = [
        {k: v for k, v in attributes_of(s).items() if k.startswith("telmoni.bound.")}
        for s in runs
    ]
    assert sent == [
        {"telmoni.bound.max_steps": 10},
        {},
        {"telmoni.bound.max_steps": 7},
        {},
        {"telmoni.bound.stall_after_s": 2**32 - 1},
        {},
        {},
    ]
    warnings = {r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING}
    whole = "must be a whole number from 1 to 4294967295; the run is sent without it"
    cost = "must be a finite number above 0; the run is sent without it"
    assert warnings == {
        f"telmoni: a run's stall_after_s {whole}",
        f"telmoni: a run's max_steps {whole}",
        f"telmoni: a run's max_cost_usd {cost}",
    }


def test_a_run_sends_its_first_32_tags_and_warns_once_of_the_rest(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Ingest keeps no more, and past the span's room they would crowd out the
    run's own: its id, or the error it ends with."""
    tags = {f"t{n:02}": "v" for n in range(40)}
    for _ in range(2):
        with telmoni.run("bot", tags=tags):
            pass
    assert telmoni.flush()
    for run in (s for s in transport.spans() if s["name"] == "invoke_agent bot"):
        sent = [k for k in attributes_of(run) if k.startswith("telmoni.tag.")]
        assert sent == [f"telmoni.tag.t{n:02}" for n in range(32)]
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warnings == [
        (
            "telmoni: a run carries up to 32 tags, the most ingest keeps; "
            "the rest are not sent"
        )
    ]


def test_a_tags_number_goes_as_it_is_another_value_as_its_text_and_none_left_off(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    class Tier(int):
        """An `(int, Enum)`'s member, its `str()` its name."""

        def __str__(self) -> str:
            return "Tier.GOLD"

    tags = {
        "team": None,
        "n": 3,
        "on": True,
        "ratio": 2.5,
        "big": 2**64,
        "odd": float("nan"),
        "tier": Tier(3),
    }
    with telmoni.run("bot", tags=tags):  # type: ignore[arg-type]
        pass
    assert telmoni.flush()
    (run,) = (s for s in transport.spans() if s["name"] == "invoke_agent bot")
    typed = {a["key"]: a["value"] for a in run["attributes"]}
    assert typed["telmoni.tag.n"] == {"intValue": "3"}
    assert typed["telmoni.tag.tier"] == {"intValue": "3"}
    assert typed["telmoni.tag.on"] == {"boolValue": True}
    assert typed["telmoni.tag.ratio"] == {"doubleValue": 2.5}
    assert typed["telmoni.tag.big"] == {"stringValue": str(2**64)}
    assert typed["telmoni.tag.odd"] == {"stringValue": "nan"}
    assert "telmoni.tag.team" not in typed
    texts = [r for r in caplog.records if "is sent as its text" in r.getMessage()]
    assert len(texts) == 1


def test_a_tag_ingest_would_drop_takes_no_place_among_the_32(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Ingest drops them before it counts its 32, so each would cost one of those."""
    tags = {"": "empty", "k" * 65: "long", "v": "x" * 257}
    tags.update({f"t{n:02}": "1" for n in range(31)})
    tags["\u00e9" * 64] = "\u00fc" * 256
    # Past the 32nd, a tag ingest drops is still warned of as dropped.
    tags["w"] = "y" * 257
    with telmoni.run("bot", tags=tags):
        pass
    assert telmoni.flush()
    (run,) = (s for s in transport.spans() if s["name"] == "invoke_agent bot")
    sent = {key for key in attributes_of(run) if key.startswith("telmoni.tag.")}
    assert sent == {f"telmoni.tag.t{n:02}" for n in range(31)} | {
        "telmoni.tag." + "\u00e9" * 64
    }
    messages = [record.getMessage() for record in caplog.records]
    assert sum("1 to 64 characters" in message for message in messages) == 1
    assert not any("up to 32 tags" in message for message in messages)


def test_a_tags_number_ingest_writes_past_256_characters_takes_no_place_either(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Ingest writes a float's every digit, never an exponent: 1e256 in 257."""
    tags: dict[str, object] = {"at": 1e255, "small": 1.5e-253}
    tags.update({"past": 1e256, "below": -1e255, "tiny": 1e-255})
    tags.update({f"t{n:02}": "1" for n in range(30)})
    with telmoni.run("bot", tags=tags):  # type: ignore[arg-type]
        pass
    assert telmoni.flush()
    (run,) = (s for s in transport.spans() if s["name"] == "invoke_agent bot")
    sent = {key for key in attributes_of(run) if key.startswith("telmoni.tag.")}
    assert sent == {"telmoni.tag.at", "telmoni.tag.small"} | {
        f"telmoni.tag.t{n:02}" for n in range(30)
    }
    messages = [record.getMessage() for record in caplog.records]
    assert sum("1 to 64 characters" in message for message in messages) == 1
    assert not any("up to 32 tags" in message for message in messages)


def test_a_warning_whose_handler_raises_never_reaches_the_agent(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    """The warning is the SDK's, and so is its handler's failure."""

    class Raising(logging.Handler):
        def handle(self, record: logging.LogRecord) -> bool:
            raise RuntimeError("the handler's own")

    handler = Raising()
    logging.getLogger("telmoni").addHandler(handler)
    try:
        with telmoni.step("alone"):
            pass
        with (
            telmoni.run("bot", tags={f"t{n}": "1" for n in range(40)}),
            telmoni.step(7),  # type: ignore[arg-type]
        ):
            pass
    finally:
        logging.getLogger("telmoni").removeHandler(handler)
    assert telmoni.flush()
    assert {"invoke_agent bot", "7"} <= {span["name"] for span in transport.spans()}


def test_a_run_whose_agent_is_none_or_blank_as_text_is_refused_as_a_blank_one(
    telmoni: Telmoni,
) -> None:
    class Blank:
        def __str__(self) -> str:
            return " "

    for agent in (None, Blank()):
        with (
            pytest.raises(ValueError, match="agent's name"),
            telmoni.run(agent),  # type: ignore[arg-type]
        ):
            pass


def test_a_value_whose_own_str_raises_goes_as_its_class_name(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    class Unprintable:
        def __str__(self) -> str:
            raise RuntimeError("its own")

    with (
        telmoni.run("bot", tags={"odd": Unprintable()}),  # type: ignore[dict-item]
        telmoni.step(Unprintable()),  # type: ignore[arg-type]
    ):
        pass
    assert telmoni.flush()
    assert "Unprintable" in {span["name"] for span in transport.spans()}
    assert attributes_of(run_span(transport))["telmoni.tag.odd"] == "Unprintable"


def test_a_user_or_session_that_is_no_string_goes_as_its_text(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """As is, a number past 64 bits would lose the batch, and a tuple the run."""
    with telmoni.run(
        "bot",
        user_id=2**64,  # type: ignore[arg-type]
        session_id=("conv", 7),  # type: ignore[arg-type]
    ):
        pass
    assert telmoni.flush()
    sent = attributes_of(run_span(transport))
    assert sent["user.id"] == str(2**64)
    assert sent["gen_ai.conversation.id"] == "('conv', 7)"
    texts = {r.getMessage() for r in caplog.records if "as its text" in r.getMessage()}
    assert texts == {
        "telmoni: a user id that is no string is sent as its text",
        "telmoni: a session id that is no string is sent as its text",
    }


class RateLimited(Exception):
    pass


class Blank:
    def __str__(self) -> str:
        return ""


def test_an_error_type_that_is_no_string_goes_as_its_class_or_its_text(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """An exception's message is content; as given, the rest would raise."""
    given: list[object] = [
        RateLimited("quota exceeded for alice@example.com"),
        RateLimited(),
        RateLimited,
        429,
        Blank(),
    ]
    for error_type in given:
        with telmoni.run("bot") as run:
            run.fail(error_type)  # type: ignore[arg-type]
    assert telmoni.flush()
    runs = [s for s in transport.spans() if s["name"] == "invoke_agent bot"]
    assert [attributes_of(s).get("error.type") for s in runs] == [
        "RateLimited",
        "RateLimited",
        "RateLimited",
        "429",
        None,
    ]
    assert all(s["status"] == {"code": 2} for s in runs)
    assert "alice@example.com" not in str(transport.requests)
    texts = {r.getMessage() for r in caplog.records if "as its text" in r.getMessage()}
    assert texts == {
        "telmoni: a span's error type that is no string is sent as its text"
    }


def test_a_runs_text_past_ingests_caps_is_cut_or_left_off_warned_once(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """On every span of the run, one past its cap cost ingest the run whole."""
    long_error = type("E" * 129, (Exception,), {})
    for _ in range(2):
        with (
            pytest.raises(long_error),
            telmoni.run("😀" * 129, user_id="u" * 129, session_id="s" * 201) as run,
        ):
            assert run.agent == "😀" * 128
            raise long_error
    assert telmoni.flush()
    for span in transport.spans():
        sent = attributes_of(span)
        assert sent["telmoni.agent"] == "😀" * 128
        assert "user.id" not in sent and "gen_ai.conversation.id" not in sent
    runs = [s for s in transport.spans() if s["name"] == "invoke_agent " + "😀" * 128]
    assert len(runs) == 2
    assert all(attributes_of(s)["gen_ai.agent.name"] == "😀" * 128 for s in runs)
    assert all(attributes_of(s)["error.type"] == "E" * 128 for s in runs)
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert sorted(warnings) == [
        (
            "telmoni: a run's agent must be up to 128 characters; "
            "it is sent cut to its first 128"
        ),
        (
            "telmoni: a session id must be up to 200 characters; "
            "the run is sent without it"
        ),
        (
            "telmoni: a span's error type must be up to 128 characters; "
            "it is sent cut to its first 128"
        ),
        "telmoni: a user id must be up to 128 characters; the run is sent without it",
    ]


def test_a_runs_text_at_ingests_caps_is_sent_whole(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Counted in characters, as ingest counts them, never in UTF-8's bytes."""
    with telmoni.run("😀" * 128, user_id="😀" * 128, session_id="😀" * 200) as run:
        run.fail("😀" * 128)
    assert telmoni.flush()
    sent = attributes_of(run_span(transport))
    assert sent["telmoni.agent"] == sent["user.id"] == sent["error.type"] == "😀" * 128
    assert sent["gen_ai.conversation.id"] == "😀" * 200
    assert not [r for r in caplog.records if r.levelno >= logging.WARNING]
