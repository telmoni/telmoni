from __future__ import annotations

import base64
import hashlib
import json
import logging
from dataclasses import dataclass
from datetime import datetime, timezone
from enum import Enum

import pytest

from conftest import ENDPOINT, FakeTransport, attributes_of, infos_in, kept
from telmoni import (
    Config,
    ModelCallRecord,
    Telmoni,
    TraceRecord,
    Usage,
    WarningInfo,
)
from telmoni.exporter import Answer


def calls(transport: FakeTransport) -> list[dict[str, object]]:
    return [s for s in transport.spans() if s["name"].startswith("chat ")]


def test_reasoning_survives_partial_usage_updates(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.trace("bot"), telmoni.model_call("m") as call:
        call.answered(usage=Usage(output_tokens=7, reasoning_output_tokens=4))
        call.answered(usage=Usage(reasoning_output_tokens=2))
        call.answered(usage=Usage(input_tokens=12))
        call.answered(usage=Usage(reasoning_output_tokens=-1))
    assert telmoni.flush()
    attributes = attributes_of(calls(transport)[0])
    assert attributes["gen_ai.usage.output_tokens"] == 7
    assert attributes["gen_ai.usage.reasoning.output_tokens"] == 2
    assert attributes["gen_ai.usage.input_tokens"] == 12


def test_a_model_call_carries_the_conventions_names(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with (
        telmoni.trace("bot", session_id="conv") as trace,
        telmoni.model_call("gpt-4o-mini", provider="openai") as call,
    ):
        call.answered(
            response_model="gpt-4o-mini-2024-07-18",
            usage=Usage(input_tokens=12, output_tokens=7, cache_read_input_tokens=3),
        )
    assert telmoni.flush()
    (span,) = calls(transport)
    assert span["name"] == "chat gpt-4o-mini" and span["kind"] == 3
    assert span["traceId"] == trace.trace_id
    assert attributes_of(span) == {
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
    with telmoni.trace("bot"):
        for usage in usages:
            with telmoni.model_call("m") as call:
                call.answered(usage=usage)
    at = datetime(2026, 10, 18, 9, tzinfo=timezone.utc)
    telmoni.record_trace(
        TraceRecord(
            trace_id="3d6f8a1c5b2e4f7a9c0d8e1f2a3b4c5d",
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
                "gen_ai.usage.cache_creation.input_tokens",
            }
            & typed.keys()
        )
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert len(warnings) == 3, warnings
    assert all("must be a whole number from 0 within 64 bits" in w for w in warnings)


def test_a_calls_time_to_first_token_goes_in_seconds(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.trace("bot") as trace:
        with trace.model_call("m") as call:
            call.answered(time_to_first_token=0.35)
        with telmoni.model_call("m") as call:
            call.answered()
    assert telmoni.flush()
    timed, bare = (
        {a["key"]: a["value"] for a in span["attributes"]}
        for span in transport.spans()
        if span["name"] == "chat m"
    )
    assert timed["gen_ai.response.time_to_first_chunk"] == {"doubleValue": 0.35}
    assert "gen_ai.response.time_to_first_chunk" not in bare


def test_a_time_to_first_token_ingest_cannot_take_is_left_off(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Ingest refuses the call over a time to first token below 0 or not finite."""
    with telmoni.trace("bot"):
        with telmoni.model_call("m") as call:
            call.answered(response_model="m-1", time_to_first_token=-0.1)
        with telmoni.model_call("m") as call:
            call.answered(time_to_first_token=float("nan"))
    assert telmoni.flush()
    spans = calls(transport)
    assert len(spans) == 2, "each call still sent"
    first, second = (attributes_of(span) for span in spans)
    assert first["gen_ai.response.model"] == "m-1"
    assert "gen_ai.response.time_to_first_chunk" not in {**first, **second}
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warnings == [
        (
            "telmoni: gen_ai.response.time_to_first_chunk must be a finite number of "
            "seconds from 0; the call is sent without it"
        )
    ]
    assert infos_in(caplog) == [WarningInfo("input")]


@dataclass(frozen=True)
class AnthropicUsage:
    """Anthropic's usage, which names its cache writes otherwise and no cost."""

    input_tokens: int
    output_tokens: int
    cache_creation_input_tokens: int | None = None
    cache_read_input_tokens: int | None = None


@dataclass(frozen=True)
class CompletionUsage:
    """OpenAI's usage, whose counts go by other names."""

    prompt_tokens: int
    completion_tokens: int
    total_tokens: int


def test_a_usage_that_is_not_a_usage_is_taken_as_none_with_one_warning(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """A provider's own usage, handed as it came, never raises into the turn."""
    anthropic = AnthropicUsage(input_tokens=12, output_tokens=7)
    openai = CompletionUsage(prompt_tokens=12, completion_tokens=7, total_tokens=19)
    with telmoni.trace("bot", max_spend=1.0) as trace:
        with telmoni.model_call("m", provider="anthropic") as call:
            call.answered(response_model="m-1", usage=anthropic)  # type: ignore[arg-type]
        with telmoni.model_call("m", provider="openai") as call:
            call.answered(usage=openai)  # type: ignore[arg-type]
        spent = trace.spent
    assert telmoni.flush()
    first, second = (attributes_of(span) for span in calls(transport))
    assert first["gen_ai.response.model"] == "m-1"
    assert not [key for key in {**first, **second} if key.startswith("gen_ai.usage.")]
    assert spent == 0
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warnings == [
        (
            "telmoni: a usage must be telmoni's Usage, the provider's counts copied "
            "into it; the call is sent without it"
        )
    ]


def test_a_blank_operation_is_chat(telmoni: Telmoni, transport: FakeTransport) -> None:
    # As one left out is: sent blank, the span would be named " m" and ingest
    # would read it as a step. One that is no string is no operation either.
    with telmoni.trace("bot") as trace:
        with telmoni.model_call("m", operation=""):
            pass
        with trace.model_call("m", operation="  "):
            pass
        with trace.model_call("m", operation=5):  # type: ignore[arg-type]
            pass
    assert telmoni.flush()
    spans = [s for s in transport.spans() if s["name"] == "chat m"]
    assert len(spans) == 3, [s["name"] for s in transport.spans()]
    assert all(attributes_of(s)["gen_ai.operation.name"] == "chat" for s in spans)


def test_a_provider_or_response_model_goes_as_text_and_an_empty_one_not_at_all(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Ingest refuses the call over an empty one, or over a tuple."""
    with telmoni.trace("bot"):
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
    """Ingest refuses a call over an empty model, or text past its cap, and files
    one whose operation is none of its four as no model call."""
    with telmoni.trace("bot") as trace:
        for _ in range(2):
            with trace.model_call(""):
                pass
            with trace.model_call(
                "m" * 129, provider="p" * 129, operation="generate_content"
            ) as call:
                call.answered(response_model="r" * 129)
            with trace.model_call("m" * 128, operation="o" * 20):
                pass
        # At their caps: the longest operation and a model at its cap fit.
        with trace.model_call("😀" * 128, operation="generate_content") as call:
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
        "chat " + "m" * 128,
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
            "telmoni: a model call's operation must be chat, generate_content, "
            "text_completion or embeddings, which ingest reads as a model call; "
            "it is sent as chat"
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


def prompts_of(span: dict[str, object]) -> dict[str, object]:
    return {
        key: value
        for key, value in attributes_of(span).items()
        if key.startswith("gen_ai.prompt.")
    }


def test_a_prompt_goes_on_its_call_alone_and_a_recorded_call_names_none(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with (
        telmoni.trace("bot") as trace,
        telmoni.step("plan"),
        trace.model_call("m", prompt="support-reply", prompt_version="3"),
    ):
        pass
    at = datetime(2026, 10, 18, 9, tzinfo=timezone.utc)
    telmoni.record_trace(
        TraceRecord(
            trace_id="3d6f8a1c5b2e4f7a9c0d8e1f2a3b4c5d",
            agent="recorder",
            start=at,
            end=at,
            inside=[ModelCallRecord(model="r", start=at, end=at)],
        )
    )
    assert telmoni.flush()
    assert {span["name"]: prompts_of(span) for span in transport.spans()} == {
        "invoke_agent bot": {},
        "plan": {},
        "chat m": {"gen_ai.prompt.name": "support-reply", "gen_ai.prompt.version": "3"},
        "invoke_agent recorder": {},
        "chat r": {},
    }


def test_a_blank_prompt_is_none_and_a_version_without_its_name_is_left_off(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """A blank one is none, as an empty provider is; a version alone names no
    prompt."""
    with telmoni.trace("bot") as trace:
        for prompt, version in (
            ("", ""),
            (" ", "\N{IDEOGRAPHIC SPACE}"),
            ("p", "\t"),
            ("", "3"),
            (None, "3"),
            ("\N{EM SPACE}", "3"),
        ):
            with trace.model_call("m", prompt=prompt, prompt_version=version):
                pass
    assert telmoni.flush()
    sent = [prompts_of(span) for span in calls(transport)]
    assert sent == [{}, {}, {"gen_ai.prompt.name": "p"}, {}, {}, {}]
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warnings == [
        (
            "telmoni: a prompt's version needs its prompt's name; "
            "the call is sent without it"
        )
    ]
    assert infos_in(caplog) == [WarningInfo("input")]


def test_a_prompt_past_ingests_cap_is_left_off_never_cut(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Cut, a prompt or its version would name another, as a cut user would."""
    widest = "😀" * 1024
    with telmoni.trace("bot") as trace:
        for _ in range(2):
            with trace.model_call("m", prompt=widest, prompt_version="v" * 1024):
                pass
            with trace.model_call("m", prompt="p" * 1025, prompt_version="3"):
                pass
            with trace.model_call("m", prompt="p", prompt_version="😀" * 1025):
                pass
    assert telmoni.flush()
    sent = [prompts_of(span) for span in calls(transport)]
    assert (
        sent
        == [
            {"gen_ai.prompt.name": widest, "gen_ai.prompt.version": "v" * 1024},
            {},
            {"gen_ai.prompt.name": "p"},
        ]
        * 2
    )
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert sorted(warnings) == [
        (
            "telmoni: a prompt's name must be up to 1024 characters; "
            "the call is sent without it"
        ),
        (
            "telmoni: a prompt's version must be up to 1024 characters; "
            "the call is sent without it"
        ),
        (
            "telmoni: a prompt's version needs its prompt's name; "
            "the call is sent without it"
        ),
    ]
    assert infos_in(caplog) == [WarningInfo("input")] * 3


class Prompt(Enum):
    REPLY = "support-reply"


def test_a_prompt_that_is_no_string_goes_as_its_text(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    with (
        telmoni.trace("bot") as trace,
        trace.model_call(
            "m",
            prompt=Prompt.REPLY,  # type: ignore[arg-type]
            prompt_version=3,  # type: ignore[arg-type]
        ),
    ):
        pass
    assert telmoni.flush()
    (call,) = calls(transport)
    assert prompts_of(call) == {
        "gen_ai.prompt.name": "Prompt.REPLY",
        "gen_ai.prompt.version": "3",
    }
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warnings == [
        "telmoni: a prompt's name that is no string is sent as its text",
        "telmoni: a prompt's version that is no string is sent as its text",
    ]


def test_a_prompt_leaves_as_given_where_content_does_never_masked_or_scrubbed(
    transport: FakeTransport,
) -> None:
    """Metadata, as a name is: the mask and the scrubbers read content alone."""
    handed: list[str] = []

    def mask(key: str, value: str) -> str:
        handed.append(key)
        return value

    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    transport.answers = [on, on]
    client = Telmoni(
        Config(
            endpoint=ENDPOINT,
            api_key="k",
            capture_content=True,
            mask=mask,
            scrub_pii=True,
        ),
        transport=transport,
    )
    try:
        with client.trace("bot"):
            pass
        assert client.flush()
        with (
            client.trace("bot"),
            client.model_call(
                "m",
                prompt="reply-to-ada@example.com",
                prompt_version="3",
                input_messages="write to ada@example.com",
            ),
        ):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    (call,) = calls(transport)
    assert attributes_of(call)["gen_ai.input.messages"] == "write to [pii: email]"
    assert prompts_of(call) == {
        "gen_ai.prompt.name": "reply-to-ada@example.com",
        "gen_ai.prompt.version": "3",
    }
    assert set(handed) == {"gen_ai.input.messages"}


def test_content_is_taken_off_before_it_leaves(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with (
        telmoni.trace("bot"),
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


def test_a_content_mode_with_blanks_about_it_is_read_trimmed(
    transport: FakeTransport,
) -> None:
    """http.client keeps a header's trailing blanks; every SDK trims them."""
    transport.answers = [Answer(200, {"Telmoni-Content-Mode": " on\t"}, b"{}")]
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        with client.trace("bot"):
            pass
        assert client.flush()
        assert client.content_mode == "on"
    finally:
        client.shutdown()


def test_a_mode_ingest_does_not_send_keeps_content_home(
    transport: FakeTransport,
) -> None:
    """A word ingest does not send reads as no mode, after an `on` too, so
    content stays home."""
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    sealed = Answer(200, {"Telmoni-Content-Mode": "sealed"}, b"{}")
    transport.answers = [on, sealed, sealed]
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k", capture_content=True),
        transport=transport,
    )
    try:
        for _ in range(2):
            with client.trace("bot"):
                pass
            assert client.flush()
        assert client.content_mode is None
        with client.trace("bot"), client.model_call("m", input_messages="kept home"):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    assert "kept home" not in str(transport.requests)


def test_content_leaves_only_when_asked_and_the_project_allows(
    transport: FakeTransport,
) -> None:
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b'{"partialSuccess":null}')
    # An empty trace's export is answered `off`, the next two traces' `on`; each
    # trace flushed before the next opens, the mode each call is encoded under
    # is the same on every machine.
    transport.answers = [kept(), on, on]
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k", capture_content=True),
        transport=transport,
    )
    try:
        with client.trace("bot"):
            pass
        assert client.flush()
        assert client.content_mode == "off"
        with client.trace("bot"), client.model_call("m", input_messages="first"):
            pass
        assert client.flush()
        assert client.content_mode == "on"
        with client.trace("bot"), client.model_call("m", input_messages="second"):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    first, second = calls(transport)
    assert "gen_ai.input.messages" not in attributes_of(first)
    assert attributes_of(second)["gen_ai.input.messages"] == "second"


def test_content_goes_as_compact_utf8_json(transport: FakeTransport) -> None:
    """As the other three SDKs send it: no spaces, and no character escaped."""
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    transport.answers = [on, on]
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k", capture_content=True),
        transport=transport,
    )
    messages = [{"role": "user", "content": "héllo, 世界 🙂"}]
    try:
        with client.trace("bot"):
            pass
        assert client.flush()
        with client.trace("bot"), client.model_call("m", input_messages=messages):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    (call,) = calls(transport)
    assert (
        attributes_of(call)["gen_ai.input.messages"]
        == '[{"role":"user","content":"héllo, 世界 🙂"}]'
    )


def test_content_stays_home_without_capture_content_even_when_allowed(
    transport: FakeTransport,
) -> None:
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    transport.answers = [on] * 2
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        with client.trace("bot"):
            pass
        assert client.flush()
        with client.trace("bot"), client.model_call("m", input_messages="never"):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    assert "never" not in str(transport.requests)


def test_a_failed_call_records_the_error_class(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.trace("bot"):
        try:
            with telmoni.model_call("m", operation="generate_content"):
                raise ConnectionError("provider down")
        except ConnectionError:
            pass
    assert telmoni.flush()
    (span,) = [s for s in transport.spans() if s["name"] == "generate_content m"]
    assert span["status"] == {"code": 2}
    assert attributes_of(span)["error.type"] == "ConnectionError"


PNG = base64.b64decode("iVBORw0KGgo=")


def blob(data: bytes) -> list[dict[str, object]]:
    """A user message holding one image, as the conventions carry it."""
    return [
        {
            "role": "user",
            "parts": [
                {
                    "type": "blob",
                    "modality": "image",
                    "mime_type": "image/png",
                    "content": base64.b64encode(data).decode(),
                }
            ],
        }
    ]


def file_part(data: bytes) -> dict[str, object]:
    return {
        "type": "file",
        "modality": "image",
        "mime_type": "image/png",
        "file_id": f"telmoni:{hashlib.sha256(data).hexdigest()}",
    }


def sent_parts(call: dict[str, object]) -> object:
    messages = attributes_of(call)["gen_ai.input.messages"]
    assert isinstance(messages, str)
    return json.loads(messages)[0]["parts"]


def media_client(transport: FakeTransport) -> Telmoni:
    """A client asked for content whose project keeps it, the mode learned."""
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    transport.answers = [on] * 4
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k", capture_content=True),
        transport=transport,
    )
    with client.trace("warm"):
        pass
    assert client.flush()
    return client


def test_an_image_is_put_once_with_its_type_and_key(transport: FakeTransport) -> None:
    client = media_client(transport)
    try:
        with client.trace("bot"):
            for _ in range(2):
                with client.model_call("m", input_messages=blob(PNG)):
                    pass
        assert client.flush()
    finally:
        client.shutdown()
    sha256 = hashlib.sha256(PNG).hexdigest()
    ((url, headers, body),) = transport.uploads
    assert url == f"{client.config.endpoint}/v1/media/{sha256}"
    assert headers["Authorization"] == "Bearer k"
    assert headers["Content-Type"] == "image/png"
    assert body == PNG
    first, second = calls(transport)
    assert sent_parts(first) == sent_parts(second) == [file_part(PNG)]


@pytest.mark.parametrize(
    "failure", [Answer(413, {}, b"{}"), OSError("refused")], ids=["413", "unreached"]
)
def test_a_failed_upload_still_names_the_file_and_warns_once(
    transport: FakeTransport,
    caplog: pytest.LogCaptureFixture,
    failure: Answer | Exception,
) -> None:
    transport.upload_answers = [failure, failure]
    client = media_client(transport)
    try:
        for _ in range(2):
            with client.trace("bot"), client.model_call("m", input_messages=blob(PNG)):
                pass
            assert client.flush()
    finally:
        client.shutdown()
    assert len(transport.uploads) == 2, "a hash is remembered only once kept"
    for call in calls(transport):
        assert sent_parts(call) == [file_part(PNG)]
    infos = [info for info in infos_in(caplog) if info.kind == "upload_failed"]
    expected = 413 if isinstance(failure, Answer) else None
    assert infos == [WarningInfo("upload_failed", expected)]


def test_a_file_past_the_cap_is_named_but_never_put(transport: FakeTransport) -> None:
    big = b"\x89" * (5 * 1024 * 1024 + 1)
    client = media_client(transport)
    try:
        with client.trace("bot"), client.model_call("m", input_messages=blob(big)):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    assert transport.uploads == []
    (call,) = calls(transport)
    assert sent_parts(call) == [file_part(big)]
