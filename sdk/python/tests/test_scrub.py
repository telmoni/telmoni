"""The scrubbers over content: ``sdk/fixtures/canary.json`` in each mode, and what a
client sends with them, each field the mask is handed scrubbed after the mask and
the image lift, and counted in ``telmoni.scrubbed``."""

from __future__ import annotations

import base64
import hashlib
import json
import time
import uuid
from collections.abc import Callable
from datetime import datetime, timezone
from typing import Any

import pytest
from opentelemetry import trace

from conftest import FakeTransport, attributes_of, load_fixture
from telmoni import Config, Telmoni, ToolCallRecord, TraceRecord, contract
from telmoni.exporter import Answer
from telmoni.scrub import scrub

CANARY = load_fixture("canary.json")
PLANTED = CANARY["planted"]
KEY = PLANTED["cloud_key"]


def content_on(
    transport: FakeTransport,
    *,
    scrub_secrets: bool | None = None,
    scrub_pii: bool | None = None,
    mask: Callable[[str, str], str | None] | None = None,
) -> Telmoni:
    """A client sending content, its project's mode heard as on from an empty trace."""
    # On to each export: a trace's spans may leave in more than one.
    transport.answers = [Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")] * 4
    client = Telmoni(
        Config(
            api_key="k",
            capture_content=True,
            scrub_secrets=scrub_secrets,
            scrub_pii=scrub_pii,
            mask=mask,
        ),
        transport=transport,
    )
    with client.trace("bot"):
        pass
    assert client.flush()
    return client


def sent_call(transport: FakeTransport) -> dict[str, Any]:
    (call,) = [span for span in transport.spans() if span["name"] == "chat gpt-4o"]
    return attributes_of(call)


@pytest.mark.parametrize("mode", sorted(CANARY["modes"]))
@pytest.mark.parametrize("case", CANARY["cases"], ids=lambda case: case["name"])
def test_each_case_is_scrubbed_as_the_fixture_says(
    case: dict[str, Any], mode: str
) -> None:
    switches = CANARY["modes"][mode]
    text, counts = scrub(case["text"], switches["scrub_secrets"], switches["scrub_pii"])
    assert text == case["expect"][mode]["text"]
    assert counts == case["expect"][mode]["scrubbed"]


@pytest.mark.parametrize("mode", sorted(CANARY["modes"]))
def test_a_client_sends_a_message_scrubbed_and_counts_what_it_took_out(
    transport: FakeTransport, mode: str
) -> None:
    """Every planted value in one message, sent as the mode scrubs it."""
    switches = CANARY["modes"][mode]
    planted = CANARY["cases"][0]
    client = content_on(
        transport,
        scrub_secrets=switches["scrub_secrets"],
        scrub_pii=switches["scrub_pii"],
    )
    messages = [
        {"role": "user", "parts": [{"type": "text", "content": planted["text"]}]}
    ]
    try:
        with (
            client.trace("support-bot"),
            client.model_call("gpt-4o", input_messages=messages),
        ):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    sent = sent_call(transport)
    (part,) = json.loads(sent[contract.INPUT_MESSAGES])[0]["parts"]
    assert part["content"] == planted["expect"][mode]["text"]
    counts = planted["expect"][mode]["scrubbed"]
    if counts:
        assert json.loads(sent[contract.SCRUBBED]) == counts
    else:
        assert contract.SCRUBBED not in sent


class Declined(Exception):
    pass


def test_a_failure_and_an_event_are_scrubbed_where_the_mask_reads_them(
    transport: FakeTransport,
) -> None:
    """The exception's message and stack trace, the status's message and an
    application's event, counted together on the agent's span."""
    client = content_on(transport)
    message = f"declined for key {KEY}"
    try:
        with pytest.raises(Declined), client.trace("support-bot"):
            trace.get_current_span().add_event(
                "retrying", {"reason": f"key {KEY}", "attempt": 2}
            )
            raise Declined(message)
        assert client.flush()
    finally:
        client.shutdown()
    agent = transport.spans()[-1]
    events = {event["name"]: attributes_of(event) for event in agent["events"]}
    assert events["retrying"] == {"reason": "key [secret: aws_key]", "attempt": 2}
    failure = events["exception"]
    assert failure["exception.message"] == "declined for key [secret: aws_key]"
    assert "declined for key [secret: aws_key]" in failure["exception.stacktrace"]
    assert agent["status"]["message"] == "declined for key [secret: aws_key]"
    assert json.loads(attributes_of(agent)[contract.SCRUBBED]) == {"secret:aws_key": 4}
    assert KEY not in str(transport.requests)


def test_a_recorded_traces_content_is_scrubbed_as_a_live_traces_is(
    transport: FakeTransport,
) -> None:
    client = content_on(transport)
    at = datetime(2026, 11, 5, 9, tzinfo=timezone.utc)
    tool = ToolCallRecord(
        name="psql",
        start=at,
        end=at,
        arguments={"password": "Hunter2-canary!"},
        result=f"connected to {PLANTED['connection_string']}",
    )
    try:
        client.record_trace(
            TraceRecord(
                trace_id=uuid.uuid4().hex,
                agent="claude-code",
                start=at,
                end=at,
                inside=[tool],
            )
        )
        assert client.flush()
    finally:
        client.shutdown()
    (span,) = [s for s in transport.spans() if s["name"] == "execute_tool psql"]
    sent = attributes_of(span)
    assert sent[contract.TOOL_CALL_ARGUMENTS] == '{"password":"[secret: password]"}'
    assert sent[contract.TOOL_CALL_RESULT] == "connected to [secret: connection_string]"
    assert json.loads(sent[contract.SCRUBBED]) == {
        "secret:connection_string": 1,
        "secret:password": 1,
    }


@pytest.mark.parametrize(
    ("scrub_pii", "image"),
    [(False, "AKIAIOSFODNN7EXAMPLE"), (True, "/home/dana/AAAAA")],
)
def test_an_image_is_lifted_out_before_its_message_is_scrubbed(
    transport: FakeTransport, scrub_pii: bool, image: str
) -> None:
    """Its base64, read as text, would match a rule and break the image."""
    client = content_on(transport, scrub_pii=scrub_pii)
    blob = {"type": "blob", "modality": "image", "mime_type": "image/png"}
    messages = [
        {
            "role": "user",
            "parts": [
                {"type": "text", "content": f"key {KEY}"},
                {**blob, "content": image},
            ],
        }
    ]
    try:
        with client.trace("bot"), client.model_call("gpt-4o", input_messages=messages):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    data = base64.b64decode(image)
    assert [body for _, _, body in transport.uploads] == [data]
    sent = sent_call(transport)
    file = {
        **blob,
        "type": "file",
        "file_id": f"telmoni:{hashlib.sha256(data).hexdigest()}",
    }
    assert json.loads(sent[contract.INPUT_MESSAGES]) == [
        {
            "role": "user",
            "parts": [{"type": "text", "content": "key [secret: aws_key]"}, file],
        }
    ]
    assert json.loads(sent[contract.SCRUBBED]) == {"secret:aws_key": 1}


def test_the_scrubbers_read_what_the_mask_answered_and_never_what_it_dropped(
    transport: FakeTransport,
) -> None:
    def mask(key: str, value: str) -> str | None:
        if key == contract.OUTPUT_MESSAGES:
            return None
        return value.replace("the card", f"the key {KEY}")

    client = content_on(transport, mask=mask)
    try:
        with (
            client.trace("bot"),
            client.model_call("gpt-4o", input_messages="show the card") as call,
        ):
            call.answered(output_messages=f"here is {KEY}")
        assert client.flush()
    finally:
        client.shutdown()
    sent = sent_call(transport)
    assert sent[contract.INPUT_MESSAGES] == "show the key [secret: aws_key]"
    assert contract.OUTPUT_MESSAGES not in sent
    assert json.loads(sent[contract.SCRUBBED]) == {"secret:aws_key": 1}


def test_a_field_the_scrubber_fails_on_is_dropped_and_named(
    transport: FakeTransport,
) -> None:
    """Sent without the field, never raw, and counted ``dropped:<field>``."""

    def failing(text: str, secrets: bool, pii: bool) -> tuple[str, dict[str, int]]:
        if "boom" in text:
            raise RuntimeError(text)
        return scrub(text, secrets, pii)

    client = content_on(transport)
    client.exporter._scrubber = failing
    try:
        with (
            client.trace("bot"),
            client.model_call("gpt-4o", input_messages=f"boom {KEY}") as call,
        ):
            call.answered(output_messages=f"here is {KEY}")
        assert client.flush()
    finally:
        client.shutdown()
    sent = sent_call(transport)
    assert contract.INPUT_MESSAGES not in sent
    assert sent[contract.OUTPUT_MESSAGES] == "here is [secret: aws_key]"
    assert json.loads(sent[contract.SCRUBBED]) == {
        "dropped:gen_ai.input.messages": 1,
        "secret:aws_key": 1,
    }
    assert "boom" not in str(transport.requests)


@pytest.mark.parametrize(
    ("given", "on"),
    [
        (None, True),
        ("", True),
        ("0", False),
        (" 0\t", False),
        ("\u3000 0", False),
        ("00", True),
        ("false", True),
        ("off", True),
    ],
)
def test_secrets_are_scrubbed_unless_the_environment_says_0(
    monkeypatch: pytest.MonkeyPatch, given: str | None, on: bool
) -> None:
    if given is not None:
        monkeypatch.setenv(contract.SCRUB_SECRETS_ENV, given)
    assert Config().scrub_secrets is on
    assert Config.from_env().scrub_secrets is on


@pytest.mark.parametrize(
    ("given", "on"),
    [
        (None, False),
        ("", False),
        ("1", True),
        ("\t1 ", True),
        ("0", False),
        ("01", False),
        ("true", False),
        ("yes", False),
    ],
)
def test_personal_data_is_scrubbed_only_where_the_environment_says_1(
    monkeypatch: pytest.MonkeyPatch, given: str | None, on: bool
) -> None:
    if given is not None:
        monkeypatch.setenv(contract.SCRUB_PII_ENV, given)
    assert Config().scrub_pii is on
    assert Config.from_env().scrub_pii is on


def test_a_switch_set_in_code_beats_the_environment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv(contract.SCRUB_SECRETS_ENV, "0")
    monkeypatch.setenv(contract.SCRUB_PII_ENV, "1")
    config = Config(scrub_secrets=True, scrub_pii=False)
    assert (config.scrub_secrets, config.scrub_pii) == (True, False)
    monkeypatch.delenv(contract.SCRUB_SECRETS_ENV)
    monkeypatch.delenv(contract.SCRUB_PII_ENV)
    config = Config(scrub_secrets=False, scrub_pii=True)
    assert (config.scrub_secrets, config.scrub_pii) == (False, True)


def test_a_switch_that_is_no_bool_is_refused() -> None:
    """A "0" from the environment is truthy, and would read as on."""
    with pytest.raises(TypeError, match="scrub_secrets must be a bool or None"):
        Config(scrub_secrets="0")  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="scrub_pii must be a bool or None"):
        Config(scrub_pii=1)  # type: ignore[arg-type]


def test_the_environments_switches_reach_what_is_sent(
    monkeypatch: pytest.MonkeyPatch, transport: FakeTransport
) -> None:
    monkeypatch.setenv(contract.SCRUB_SECRETS_ENV, "0")
    monkeypatch.setenv(contract.SCRUB_PII_ENV, "1")
    client = content_on(transport)
    try:
        with (
            client.trace("bot"),
            client.model_call("gpt-4o", input_messages=f"{KEY} {PLANTED['email']}"),
        ):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    sent = sent_call(transport)
    assert sent[contract.INPUT_MESSAGES] == f"{KEY} [pii: email]"
    assert json.loads(sent[contract.SCRUBBED]) == {"pii:email": 1}


HOSTILE = [
    ("eyJ-", ""),
    ("eyJ", ""),
    ("token=", ""),
    ("password=", ""),
    ("atm_", ""),
    ("xsk-ant-", ""),
    ("xoxb-", "_"),
    ("a", ""),
    ("a@", ""),
    ("a@b.", ""),
    ("x://", ""),
    ("a:b", " "),
    ("a:b ", ""),
    ("1", ""),
    ("1 ", ""),
    ("2 ", ""),
    ("1.", ""),
    ("a:", ""),
    ("/home/", ""),
    ("AB12", ""),
    ("-----BEGIN PRIVATE KEY-----", ""),
    ("+1 ", ""),
    ('-----BEGIN PRIVATE KEY-----"', ""),
    ("-----BEGIN PRIVATE KEY-----x-----END PRIVATE KEY-----", '"'),
]
"""Units each repeated past 256 KiB, then their tail: content an agent fetched can be
written to stall a scrubber that reads a run again from each start in it."""


@pytest.mark.parametrize(("unit", "tail"), HOSTILE)
def test_text_written_to_stall_a_scrubber_is_scrubbed_in_linear_time(
    unit: str, tail: str
) -> None:
    """Under the 5 s every Python scrubber is held to, which quadratic work misses
    by orders of magnitude."""
    text = unit * (262_144 // len(unit) + 1) + tail
    for mode in ("sdk_default", "plugin_default"):
        switches = CANARY["modes"][mode]
        began = time.perf_counter()
        scrub(text, switches["scrub_secrets"], switches["scrub_pii"])
        assert time.perf_counter() - began < 5, mode
