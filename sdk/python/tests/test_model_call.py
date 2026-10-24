from __future__ import annotations

import logging
from datetime import datetime, timezone

import pytest

from conftest import FakeTransport, attributes_of, kept
from telmoni import Config, ModelCallRecord, RunRecord, Telmoni, Usage
from telmoni.exporter import Answer


def calls(transport: FakeTransport) -> list[dict[str, object]]:
    return [s for s in transport.spans() if s["name"].startswith("chat ")]


def test_a_model_call_carries_the_conventions_names(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with (
        telmoni.run("bot", session_id="conv") as run,
        telmoni.model_call("gpt-4o-mini", provider="openai") as call,
    ):
        call.answered(
            response_model="gpt-4o-mini-2024-07-18",
            usage=Usage(input_tokens=12, output_tokens=7, cache_read_input_tokens=3),
        )
    assert telmoni.flush()
    (span,) = calls(transport)
    assert span["name"] == "chat gpt-4o-mini" and span["kind"] == 3
    assert attributes_of(span) == {
        "telmoni.run_id": run.run_id,
        "telmoni.agent": "bot",
        "gen_ai.conversation.id": "conv",
        "gen_ai.operation.name": "chat",
        "gen_ai.request.model": "gpt-4o-mini",
        "gen_ai.provider.name": "openai",
        "gen_ai.response.model": "gpt-4o-mini-2024-07-18",
        "gen_ai.usage.input_tokens": 12,
        "gen_ai.usage.output_tokens": 7,
        "gen_ai.usage.cache_read.input_tokens": 3,
    }


def test_a_token_count_goes_as_a_whole_number_from_0_and_another_is_left_off(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    usages = (
        Usage(
            input_tokens=float("nan"),  # type: ignore[arg-type]
            output_tokens=12.7,  # type: ignore[arg-type]
            cache_read_input_tokens=float("inf"),  # type: ignore[arg-type]
            cache_write_input_tokens=2**63,
        ),
        # Ingest refuses the call over a count below zero, or one no integer.
        Usage(
            input_tokens=-1,
            output_tokens=12,
            cache_read_input_tokens=True,
            cache_write_input_tokens=-5,
        ),
    )
    with telmoni.run("bot"):
        for usage in usages:
            with telmoni.model_call("m") as call:
                call.answered(usage=usage)
    at = datetime(2026, 10, 18, 9, tzinfo=timezone.utc)
    telmoni.record_run(
        RunRecord(
            run_id="3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d",
            agent="bot",
            start=at,
            end=at,
            inside=[
                ModelCallRecord(model="m", start=at, end=at, usage=usage)
                for usage in usages
            ],
        )
    )
    assert telmoni.flush()
    spans = [s for s in transport.spans() if s["name"] == "chat m"]
    assert len(spans) == 4, "the calls sent, recorded ones too, and the batch"
    for span in spans:
        typed = {a["key"]: a["value"] for a in span["attributes"]}
        assert typed["gen_ai.usage.output_tokens"] == {"intValue": "12"}
        assert (
            not {
                "gen_ai.usage.input_tokens",
                "gen_ai.usage.cache_read.input_tokens",
                "gen_ai.usage.cache_write.input_tokens",
            }
            & typed.keys()
        )
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert len(warnings) == 3, warnings
    assert all("must be a whole number from 0 within 64 bits" in w for w in warnings)


def test_a_blank_operation_is_chat(telmoni: Telmoni, transport: FakeTransport) -> None:
    # As one left out is: sent blank, the span would be named " m" and ingest
    # would read it as a step. One that is no string is no operation either.
    with telmoni.run("bot") as run:
        with telmoni.model_call("m", operation=""):
            pass
        with run.model_call("m", operation="  "):
            pass
        with run.model_call("m", operation=5):  # type: ignore[arg-type]
            pass
    assert telmoni.flush()
    spans = [s for s in transport.spans() if s["name"] == "chat m"]
    assert len(spans) == 3, [s["name"] for s in transport.spans()]
    assert all(attributes_of(s)["gen_ai.operation.name"] == "chat" for s in spans)


def test_a_provider_or_response_model_goes_as_text_and_an_empty_one_not_at_all(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Ingest refuses the call over an empty one, or over a tuple."""
    with telmoni.run("bot"):
        with telmoni.model_call(
            "m",
            provider=("open", "ai"),  # type: ignore[arg-type]
        ) as call:
            call.answered(response_model="")
        with telmoni.model_call("m", provider="") as call:
            call.answered(response_model=5)  # type: ignore[arg-type]
    assert telmoni.flush()
    first, second = (attributes_of(span) for span in calls(transport))
    assert first["gen_ai.provider.name"] == "('open', 'ai')"
    assert "gen_ai.response.model" not in first
    assert "gen_ai.provider.name" not in second
    assert second["gen_ai.response.model"] == "5"
    texts = {r.getMessage() for r in caplog.records if "as its text" in r.getMessage()}
    assert texts == {
        "telmoni: a provider that is no string is sent as its text",
        "telmoni: a response model that is no string is sent as its text",
    }


def test_a_model_calls_text_past_ingests_caps_keeps_its_call(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Ingest refuses a call over an empty model, or text past its cap."""
    with telmoni.run("bot") as run:
        for _ in range(2):
            with run.model_call(""):
                pass
            with run.model_call(
                "m" * 129, provider="p" * 129, operation="generate_content"
            ) as call:
                call.answered(response_model="r" * 129)
            with run.model_call("m" * 128, operation="o" * 20):
                pass
        # At their caps: the longest operation and a model at its cap fit.
        with run.model_call("😀" * 128, operation="generate_content") as call:
            call.answered(response_model="😀" * 128)
    assert telmoni.flush()
    sent = {
        s["name"]: attributes_of(s)
        for s in transport.spans()
        if s["name"] != "invoke_agent bot"
    }
    assert sent.keys() == {
        "chat",
        "generate_content " + "m" * 128,
        ("o" * 20 + " " + "m" * 128)[:145],
        "generate_content " + "😀" * 128,
    }
    assert "gen_ai.request.model" not in sent["chat"]
    assert sent["chat"]["gen_ai.operation.name"] == "chat"
    cut = sent["generate_content " + "m" * 128]
    assert cut["gen_ai.request.model"] == "m" * 128
    assert cut["gen_ai.provider.name"] == "p" * 128
    assert cut["gen_ai.response.model"] == "r" * 128
    widest = sent["generate_content " + "😀" * 128]
    assert widest["gen_ai.response.model"] == "😀" * 128
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert sorted(warnings) == [
        (
            "telmoni: a model call needs its model; one without is sent named by "
            "its operation alone"
        ),
        (
            "telmoni: a model call's name must be up to 145 characters; "
            "it is sent cut to its first 145"
        ),
        "telmoni: a model must be up to 128 characters; it is sent cut to its first 128",
        (
            "telmoni: a provider must be up to 128 characters; "
            "it is sent cut to its first 128"
        ),
        (
            "telmoni: a response model must be up to 128 characters; "
            "it is sent cut to its first 128"
        ),
    ]


def test_content_is_taken_off_before_it_leaves(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with (
        telmoni.run("bot"),
        telmoni.model_call(
            "m", input_messages=[{"role": "user", "content": "my secret prompt"}]
        ) as call,
    ):
        call.answered(output_messages=[{"role": "assistant", "content": "the answer"}])
    assert telmoni.flush()
    (span,) = calls(transport)
    keys = set(attributes_of(span))
    assert "gen_ai.input.messages" not in keys and "gen_ai.output.messages" not in keys
    assert "my secret prompt" not in str(transport.requests)


def test_content_leaves_only_when_asked_and_the_project_allows(
    transport: FakeTransport,
) -> None:
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b'{"partialSuccess":null}')
    # An empty run's export is answered `off`, the next two runs' `on`; each run
    # flushed before the next opens, the mode each call is encoded under is the
    # same on every machine.
    transport.answers = [kept(), on, on]
    client = Telmoni(Config(api_key="k", capture_content=True), transport=transport)
    try:
        with client.run("bot"):
            pass
        assert client.flush()
        assert client.content_mode == "off"
        with client.run("bot"), client.model_call("m", input_messages="first"):
            pass
        assert client.flush()
        assert client.content_mode == "on"
        with client.run("bot"), client.model_call("m", input_messages="second"):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    first, second = calls(transport)
    assert "gen_ai.input.messages" not in attributes_of(first)
    assert attributes_of(second)["gen_ai.input.messages"] == "second"


def test_content_stays_home_without_capture_content_even_when_allowed(
    transport: FakeTransport,
) -> None:
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    transport.answers = [on] * 2
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        with client.run("bot"):
            pass
        assert client.flush()
        with client.run("bot"), client.model_call("m", input_messages="never"):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    assert "never" not in str(transport.requests)


def test_a_failed_call_records_the_error_class(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.run("bot"):
        try:
            with telmoni.model_call("m", operation="generate_content"):
                raise ConnectionError("provider down")
        except ConnectionError:
            pass
    assert telmoni.flush()
    (span,) = [s for s in transport.spans() if s["name"] == "generate_content m"]
    assert span["status"] == {"code": 2}
    assert attributes_of(span)["error.type"] == "ConnectionError"
