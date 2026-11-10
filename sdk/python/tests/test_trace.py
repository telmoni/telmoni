from __future__ import annotations

import asyncio
import contextvars
import logging
import re
from collections.abc import AsyncIterator, Generator, Iterator

import pytest
from opentelemetry import trace

from conftest import FakeTransport, attributes_of, infos_in
from telmoni import Config, Telmoni, WarningInfo, current_trace
from telmoni.exporter import Answer

HEX32 = re.compile(r"^[0-9a-f]{32}$")


def agent_span(transport: FakeTransport) -> dict[str, object]:
    agents = [s for s in transport.spans() if s["name"].startswith("invoke_agent ")]
    assert len(agents) == 1
    return agents[0]


def test_a_trace_is_its_agents_span_with_the_contract_attributes(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.trace(
        "support-bot",
        user_id="user_1",
        session_id="conv_1",
        tags={"env": "test", "team": "a"},
    ) as opened:
        assert HEX32.match(opened.trace_id)
        assert opened.agent == "support-bot"
        assert current_trace() is opened
    assert current_trace() is None
    assert telmoni.flush()
    span = agent_span(transport)
    assert span["traceId"] == opened.trace_id
    assert span["kind"] == 1
    assert "parentSpanId" not in span
    assert "links" not in span
    assert "status" not in span
    assert attributes_of(span) == {
        "gen_ai.operation.name": "invoke_agent",
        "gen_ai.agent.name": "support-bot",
        "telmoni.agent": "support-bot",
        "user.id": "user_1",
        "gen_ai.conversation.id": "conv_1",
        "telmoni.tag.env": "test",
        "telmoni.tag.team": "a",
    }


def test_an_exception_fails_the_trace_with_its_class_alone(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with pytest.raises(TimeoutError), telmoni.trace("bot"):
        raise TimeoutError("the message, which is content and never sent")
    assert telmoni.flush()
    span = agent_span(transport)
    assert span["status"] == {"code": 2}
    assert attributes_of(span)["error.type"] == "TimeoutError"
    assert "never sent" not in str(transport.requests)


def test_fail_and_cancel_mark_the_trace(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.trace("bot") as opened:
        opened.fail("RateLimit")
    with telmoni.trace("bot") as opened:
        opened.cancel()
    assert telmoni.flush()
    agents = sorted(
        (s for s in transport.spans() if s["name"].startswith("invoke_agent ")),
        key=lambda s: "error.type" not in attributes_of(s),
    )
    assert attributes_of(agents[0])["error.type"] == "RateLimit" and agents[0][
        "status"
    ] == {"code": 2}
    assert (
        attributes_of(agents[1])["telmoni.cancelled"] is True
        and "status" not in agents[1]
    )


def test_a_fail_of_an_empty_type_fails_the_trace_with_none(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    """Ingest refuses a span over an empty ``error.type``: here, the agent's."""
    with telmoni.trace("bot") as opened:
        opened.fail("")
    assert telmoni.flush()
    (failed,) = (s for s in transport.spans() if s["name"] == "invoke_agent bot")
    assert "error.type" not in attributes_of(failed)
    assert failed["status"] == {"code": 2}


def content_kept(transport: FakeTransport) -> Telmoni:
    """A client asked for content whose project keeps it, the mode learned."""
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    transport.answers = [on] * 4
    client = Telmoni(Config(api_key="k", capture_content=True), transport=transport)
    with client.trace("warm"):
        pass
    assert client.flush()
    return client


def test_a_failure_leaves_with_its_message_and_trace_under_both_locks(
    transport: FakeTransport,
) -> None:
    client = content_kept(transport)
    try:
        with client.trace("bot") as opened:
            opened.fail(TimeoutError("took too long"))
        with pytest.raises(ValueError), client.trace("bot"), client.step("plan"):
            raise ValueError("bad plan")
        with client.trace("bot") as opened:
            opened.fail("RateLimit")
        assert client.flush()
    finally:
        client.shutdown()
    failed, step, ran, typed = transport.spans()[1:]
    for span, kind, message in (
        (failed, "TimeoutError", "took too long"),
        (step, "ValueError", "bad plan"),
        (ran, "ValueError", "bad plan"),
    ):
        assert span["status"] == {"code": 2, "message": message}
        (event,) = span["events"]
        sent = attributes_of(event)
        assert event["name"] == "exception"
        assert sent["exception.type"] == attributes_of(span)["error.type"] == kind
        assert sent["exception.message"] == message
        assert f"{kind}: {message}" in sent["exception.stacktrace"]
    # A type alone carries no message to record.
    assert typed["status"] == {"code": 2} and "events" not in typed


def test_a_failure_keeps_its_message_home_without_capture_content(
    transport: FakeTransport,
) -> None:
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    transport.answers = [on, on]
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        with client.trace("warm"):
            pass
        assert client.flush()
        with client.trace("bot") as opened:
            opened.fail(TimeoutError("took too long"))
        assert client.flush()
    finally:
        client.shutdown()
    span = transport.spans()[-1]
    assert span["status"] == {"code": 2} and "events" not in span
    assert attributes_of(span)["error.type"] == "TimeoutError"
    assert "took too long" not in str(transport.requests)


def test_an_applications_event_leaves_only_under_both_locks(
    transport: FakeTransport, telmoni: Telmoni
) -> None:
    keeping = FakeTransport()
    client = content_kept(keeping)
    try:
        for each in (client, telmoni):
            with each.trace("bot"):
                trace.get_current_span().add_event("cache miss", {"key": "k1"})
            assert each.flush()
    finally:
        client.shutdown()
    (event,) = keeping.spans()[-1]["events"]
    assert event["name"] == "cache miss" and attributes_of(event) == {"key": "k1"}
    assert "events" not in transport.spans()[-1]
    assert "cache miss" not in str(transport.requests)


def test_a_keyboard_interrupt_is_a_cancellation(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with pytest.raises(KeyboardInterrupt), telmoni.trace("bot"):
        raise KeyboardInterrupt
    assert telmoni.flush()
    span = agent_span(transport)
    assert attributes_of(span)["telmoni.cancelled"] is True
    assert "status" not in span


def test_the_decorator_opens_a_trace_per_call(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    seen: list[str] = []

    @telmoni.trace("bot")
    def handle(question: str) -> str:
        opened = current_trace()
        assert opened is not None
        seen.append(opened.trace_id)
        return question.upper()

    assert handle("a") == "A"
    assert handle("b") == "B"
    assert telmoni.flush()
    agents = [s for s in transport.spans() if s["name"] == "invoke_agent bot"]
    assert len(agents) == 2 and len(set(seen)) == 2
    assert {s["traceId"] for s in agents} == set(seen), "a trace each"
    assert handle.__name__ == "handle"


def test_a_decorated_generator_spans_its_iteration(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    @telmoni.trace("bot")
    def stream(n: int) -> Iterator[int]:
        for i in range(n):
            with telmoni.step(f"s{i}"):
                pass
            yield i

    assert list(stream(2)) == [0, 1]
    # Each item asked for from a copy of the consumer's context, as a thread
    # pool asks, still runs the body in the trace.
    items = stream(2)
    assert [contextvars.copy_context().run(next, items) for _ in range(2)] == [0, 1]
    assert list(items) == []
    assert telmoni.flush()
    agents = [s for s in transport.spans() if s["name"] == "invoke_agent bot"]
    steps = [s for s in transport.spans() if s["name"] in ("s0", "s1")]
    assert len(agents) == 2 and len(steps) == 4
    assert {s["parentSpanId"] for s in steps} == {a["spanId"] for a in agents}


def test_a_decorated_generator_closed_early_is_cancelled(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    @telmoni.trace("bot")
    def stream() -> Generator[int, None, None]:
        yield 1
        yield 2

    items = stream()
    assert next(items) == 1
    items.close()
    assert telmoni.flush()
    span = agent_span(transport)
    assert attributes_of(span)["telmoni.cancelled"] is True
    assert "status" not in span


def test_a_decorated_async_generator_spans_its_iteration(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    @telmoni.trace("bot")
    async def stream(n: int) -> AsyncIterator[int]:
        for i in range(n):
            async with telmoni.step(f"a{i}"):
                await asyncio.sleep(0)
            yield i

    async def main() -> list[int]:
        return [i async for i in stream(2)]

    assert asyncio.run(main()) == [0, 1]
    assert telmoni.flush()
    agent = agent_span(transport)
    steps = [s for s in transport.spans() if s["name"] in ("a0", "a1")]
    assert [s["parentSpanId"] for s in steps] == [agent["spanId"]] * 2


def test_the_decorator_and_context_manager_work_in_asyncio(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    @telmoni.trace("bot")
    async def handle() -> str:
        opened = current_trace()
        assert opened is not None
        async with telmoni.step("think"):
            await asyncio.sleep(0)
        return opened.trace_id

    async def main() -> list[str]:
        ids = [await handle()]
        async with telmoni.trace("bot") as opened:
            ids.append(opened.trace_id)
        return ids

    ids = asyncio.run(main())
    assert telmoni.flush()
    spans = transport.spans()
    assert len([s for s in spans if s["name"] == "invoke_agent bot"]) == 2
    step = next(s for s in spans if s["name"] == "think")
    assert step["traceId"] == ids[0]


def test_nothing_is_sent_outside_a_trace(
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
    assert len(warnings) == 1 and "outside a trace" in warnings[0]
    assert infos_in(caplog) == [WarningInfo("input")]


def test_a_trace_needs_an_agent(telmoni: Telmoni) -> None:
    with pytest.raises(ValueError), telmoni.trace(" "):
        pass


def test_a_trace_sends_its_first_32_tags_and_warns_once_of_the_rest(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Ingest keeps no more, and past the span's room they would crowd out the
    agent's own: its name, or the error it ends with."""
    tags = {f"t{n:02}": "v" for n in range(40)}
    for _ in range(2):
        with telmoni.trace("bot", tags=tags):
            pass
    assert telmoni.flush()
    for agent in (s for s in transport.spans() if s["name"] == "invoke_agent bot"):
        sent = [k for k in attributes_of(agent) if k.startswith("telmoni.tag.")]
        assert sent == [f"telmoni.tag.t{n:02}" for n in range(32)]
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warnings == [
        (
            "telmoni: a trace carries up to 32 tags, the most ingest keeps; "
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
    with telmoni.trace("bot", tags=tags):  # type: ignore[arg-type]
        pass
    assert telmoni.flush()
    (agent,) = (s for s in transport.spans() if s["name"] == "invoke_agent bot")
    typed = {a["key"]: a["value"] for a in agent["attributes"]}
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
    tags["é" * 64] = "ü" * 256
    # Past the 32nd, a tag ingest drops is still warned of as dropped.
    tags["w"] = "y" * 257
    with telmoni.trace("bot", tags=tags):
        pass
    assert telmoni.flush()
    (agent,) = (s for s in transport.spans() if s["name"] == "invoke_agent bot")
    sent = {key for key in attributes_of(agent) if key.startswith("telmoni.tag.")}
    assert sent == {f"telmoni.tag.t{n:02}" for n in range(31)} | {
        "telmoni.tag." + "é" * 64
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
    with telmoni.trace("bot", tags=tags):  # type: ignore[arg-type]
        pass
    assert telmoni.flush()
    (agent,) = (s for s in transport.spans() if s["name"] == "invoke_agent bot")
    sent = {key for key in attributes_of(agent) if key.startswith("telmoni.tag.")}
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
            telmoni.trace("bot", tags={f"t{n}": "1" for n in range(40)}),
            telmoni.step(7),  # type: ignore[arg-type]
        ):
            pass
    finally:
        logging.getLogger("telmoni").removeHandler(handler)
    assert telmoni.flush()
    assert {"invoke_agent bot", "7"} <= {span["name"] for span in transport.spans()}


def test_a_trace_whose_agent_is_none_or_blank_as_text_is_refused_as_a_blank_one(
    telmoni: Telmoni,
) -> None:
    class Blank:
        def __str__(self) -> str:
            return " "

    for agent in (None, Blank()):
        with (
            pytest.raises(ValueError, match="agent's name"),
            telmoni.trace(agent),  # type: ignore[arg-type]
        ):
            pass


def test_a_value_whose_own_str_raises_goes_as_its_class_name(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    class Unprintable:
        def __str__(self) -> str:
            raise RuntimeError("its own")

    with (
        telmoni.trace("bot", tags={"odd": Unprintable()}),  # type: ignore[dict-item]
        telmoni.step(Unprintable()),  # type: ignore[arg-type]
    ):
        pass
    assert telmoni.flush()
    assert "Unprintable" in {span["name"] for span in transport.spans()}
    assert attributes_of(agent_span(transport))["telmoni.tag.odd"] == "Unprintable"


def test_a_user_or_session_that_is_no_string_goes_as_its_text(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """As is, a number past 64 bits would lose the batch, and a tuple the trace."""
    with telmoni.trace(
        "bot",
        user_id=2**64,  # type: ignore[arg-type]
        session_id=("conv", 7),  # type: ignore[arg-type]
    ):
        pass
    assert telmoni.flush()
    sent = attributes_of(agent_span(transport))
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
        with telmoni.trace("bot") as opened:
            opened.fail(error_type)  # type: ignore[arg-type]
    assert telmoni.flush()
    agents = [s for s in transport.spans() if s["name"] == "invoke_agent bot"]
    assert [attributes_of(s).get("error.type") for s in agents] == [
        "RateLimited",
        "RateLimited",
        "RateLimited",
        "429",
        None,
    ]
    assert all(s["status"] == {"code": 2} for s in agents)
    assert "alice@example.com" not in str(transport.requests)
    texts = {r.getMessage() for r in caplog.records if "as its text" in r.getMessage()}
    assert texts == {
        "telmoni: a span's error type that is no string is sent as its text"
    }


def test_a_traces_text_past_ingests_caps_is_cut_or_left_off_warned_once(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """On every span of the trace, one past its cap cost ingest the trace whole."""
    long_error = type("E" * 129, (Exception,), {})
    for _ in range(2):
        with (
            pytest.raises(long_error),
            telmoni.trace(
                "😀" * 129, user_id="u" * 129, session_id="s" * 201
            ) as opened,
        ):
            assert opened.agent == "😀" * 128
            raise long_error
    assert telmoni.flush()
    for span in transport.spans():
        sent = attributes_of(span)
        assert sent["telmoni.agent"] == "😀" * 128
        assert "user.id" not in sent and "gen_ai.conversation.id" not in sent
    agents = [s for s in transport.spans() if s["name"] == "invoke_agent " + "😀" * 128]
    assert len(agents) == 2
    assert all(attributes_of(s)["gen_ai.agent.name"] == "😀" * 128 for s in agents)
    assert all(attributes_of(s)["error.type"] == "E" * 128 for s in agents)
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert sorted(warnings) == [
        (
            "telmoni: a session id must be up to 200 characters; "
            "the trace is sent without it"
        ),
        (
            "telmoni: a span's error type must be up to 128 characters; "
            "it is sent cut to its first 128"
        ),
        (
            "telmoni: a trace's agent must be up to 128 characters; "
            "it is sent cut to its first 128"
        ),
        "telmoni: a user id must be up to 128 characters; the trace is sent without it",
    ]


def test_a_traces_text_at_ingests_caps_is_sent_whole(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Counted in characters, as ingest counts them, never in UTF-8's bytes."""
    with telmoni.trace("😀" * 128, user_id="😀" * 128, session_id="😀" * 200) as opened:
        opened.fail("😀" * 128)
    assert telmoni.flush()
    sent = attributes_of(agent_span(transport))
    assert sent["telmoni.agent"] == sent["user.id"] == sent["error.type"] == "😀" * 128
    assert sent["gen_ai.conversation.id"] == "😀" * 200
    assert not [r for r in caplog.records if r.levelno >= logging.WARNING]


def test_a_traces_own_messages_leave_only_when_asked_and_the_project_allows(
    transport: FakeTransport,
) -> None:
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    transport.answers = [on, on]
    client = Telmoni(Config(api_key="k", capture_content=True), transport=transport)
    try:
        with client.trace("warm"):
            pass
        assert client.flush()
        with client.trace(
            "bot", input_messages=[{"role": "user", "content": "hi"}]
        ) as opened:
            opened.answered(output_messages=[{"role": "assistant", "content": "hello"}])
        assert client.flush()
    finally:
        client.shutdown()
    (span,) = [s for s in transport.spans() if s["name"] == "invoke_agent bot"]
    sent = attributes_of(span)
    assert sent["gen_ai.input.messages"] == '[{"role":"user","content":"hi"}]'
    assert sent["gen_ai.output.messages"] == '[{"role":"assistant","content":"hello"}]'


def test_a_traces_own_messages_stay_home_without_capture_content_even_when_allowed(
    transport: FakeTransport,
) -> None:
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    transport.answers = [on, on]
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        with client.trace("warm"):
            pass
        assert client.flush()
        assert client.content_mode == "on"
        with client.trace("bot", input_messages="never asked") as opened:
            opened.answered(output_messages="never answered")
        assert client.flush()
    finally:
        client.shutdown()
    assert "never asked" not in str(transport.requests)
    assert "never answered" not in str(transport.requests)


def test_a_traces_answer_after_it_ended_is_not_sent_with_one_warning(
    transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    transport.answers = [on, on]
    client = Telmoni(Config(api_key="k", capture_content=True), transport=transport)
    try:
        with client.trace("warm"):
            pass
        assert client.flush()
        with client.trace("bot") as opened:
            pass
        for _ in range(2):
            opened.answered(output_messages="too late")
        assert client.flush()
    finally:
        client.shutdown()
    assert "too late" not in str(transport.requests)
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warnings == [
        (
            "telmoni: a trace must be answered before it ends; an answer given after "
            "is not sent"
        )
    ]
