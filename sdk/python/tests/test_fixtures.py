"""The golden fixtures in ``sdk/fixtures``: the export every SDK must send for
the same trace, and what each must do with the server's answers."""

from __future__ import annotations

import base64
import copy
import http.client
import json
import logging
import re
import time
from typing import Any

import pytest
from opentelemetry import trace

from conftest import (
    ENDPOINT,
    FIXTURES,
    FakeTransport,
    answer_from,
    infos_in,
    kept,
    load_fixture,
)
from telmoni import Config, Telmoni, Usage, contract
from telmoni.exporter import Answer
from telmoni.processor import Batching

HEX32 = re.compile(r"^[0-9a-f]{32}$")
HEX16 = re.compile(r"^[0-9a-f]{16}$")
VERSIONED = {"telemetry.sdk.version", "telemetry.distro.version"}


def normalize(export: dict[str, Any]) -> dict[str, Any]:
    """The fixture's normalization, as its README sets it out."""
    export = copy.deepcopy(export)
    resource_spans = export["resourceSpans"]
    assert len(resource_spans) == 1
    scope_spans = resource_spans[0]["scopeSpans"]
    assert len(scope_spans) == 1
    spans = sorted(scope_spans[0]["spans"], key=lambda s: s["name"])
    trace_ids = {span["traceId"] for span in spans}
    assert len(trace_ids) == 1 and HEX32.match(next(iter(trace_ids)))
    names = {}
    for span in spans:
        assert HEX16.match(span["spanId"])
        names[span["spanId"]] = span["name"]
    for span in spans:
        span["traceId"] = "<trace>"
        span["spanId"] = f"<span:{names[span['spanId']]}>"
        if "parentSpanId" in span:
            assert span["parentSpanId"] in names, span["name"]
            span["parentSpanId"] = f"<span:{names[span['parentSpanId']]}>"
        start, end = int(span["startTimeUnixNano"]), int(span["endTimeUnixNano"])
        assert end >= start
        span["startTimeUnixNano"] = span["endTimeUnixNano"] = "<time>"
        span["attributes"] = sorted(span["attributes"], key=lambda a: a["key"])
    scope_spans[0]["spans"] = spans
    scope_spans[0]["scope"]["version"] = "<version>"
    resource = resource_spans[0]["resource"]
    resource["attributes"] = sorted(resource["attributes"], key=lambda a: a["key"])
    for attribute in resource["attributes"]:
        if attribute["key"] == "telemetry.sdk.language":
            attribute["value"]["stringValue"] = "<language>"
        elif attribute["key"] in VERSIONED:
            attribute["value"]["stringValue"] = "<version>"
    return export


def perform(telmoni: Telmoni, scenario: dict[str, Any]) -> None:
    trace_spec = scenario["trace"]
    with telmoni.trace(
        trace_spec["agent"],
        user_id=trace_spec["user_id"],
        session_id=trace_spec["session_id"],
        tags=trace_spec["tags"],
    ):
        step_spec = trace_spec["inside"][0]
        with telmoni.step(step_spec["step"]):
            call_spec = step_spec["inside"][0]["model_call"]
            with telmoni.model_call(
                call_spec["model"],
                provider=call_spec["provider"],
                prompt=call_spec["prompt"],
                prompt_version=call_spec["prompt_version"],
                input_messages=call_spec["input_messages"],
            ) as call:
                call.answered(
                    response_model=call_spec["response_model"],
                    usage=Usage(**call_spec["usage"]),
                    output_messages=call_spec["output_messages"],
                    time_to_first_token=call_spec["time_to_first_token"],
                )
        tool_spec = trace_spec["inside"][1]["tool_call"]
        with telmoni.tool_call(tool_spec["name"]) as tool:
            tool.fail(tool_spec["fails_with"])
    assert telmoni.flush()


def test_the_trace_matches_the_golden_export(
    transport: FakeTransport, sleeps: list[float]
) -> None:
    fixture = load_fixture("trace.json")
    scenario = fixture["scenario"]
    telmoni = Telmoni(
        Config(
            endpoint=ENDPOINT,
            api_key="telmoni_test_key",
            capture_content=scenario["capture_content"],
            environment=scenario["environment"],
            release=scenario["release"],
        ),
        transport=transport,
        sleep=sleeps.append,
    )
    try:
        perform(telmoni, scenario)
    finally:
        telmoni.shutdown()
    bodies = transport.bodies()
    assert bodies, "the trace was sent"
    gathered = copy.deepcopy(bodies[0])
    gathered["resourceSpans"][0]["scopeSpans"][0]["spans"] = [
        span
        for body in [*bodies, *transport.openings]
        for span in body["resourceSpans"][0]["scopeSpans"][0]["spans"]
    ]
    assert normalize(gathered) == fixture["export"]
    for _url, headers, _body in transport.requests:
        assert headers["Authorization"] == "Bearer telmoni_test_key"
        assert headers["Content-Type"] == "application/json"
        assert headers["User-Agent"].startswith("telmoni-python/")
        assert headers["Telmoni-Sent-At"].isdigit()


@pytest.mark.parametrize("case", load_fixture("retrieval.json")["cases"])
def test_the_retrievals_match_the_golden_export(
    case: dict[str, Any], transport: FakeTransport
) -> None:
    fixture = load_fixture("retrieval.json")
    spec = fixture["scenario"]["trace"]
    mode = Answer(200, {"Telmoni-Content-Mode": case["content_mode"]}, b"{}")
    transport.answers = [mode] * 4
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k", capture_content=case["capture_content"]),
        transport=transport,
    )
    try:
        with client.trace(
            spec["agent"], user_id=spec["user_id"], session_id=spec["session_id"]
        ):
            for item in spec["inside"]:
                each = item["retrieval"]
                with client.retrieval(
                    each["source"],
                    query=each["query"],
                    top_k=each["top_k"],
                    memory=each.get("memory", False),
                ) as found:
                    if "documents" in each:
                        found.returned(each["documents"])
                    else:
                        found.fail(each["fails_with"])
        assert client.flush()
    finally:
        client.shutdown()
    # The ask before the first content holds no spans.
    bodies = [body for body in transport.bodies() if body["resourceSpans"]]
    gathered = copy.deepcopy(bodies[0])
    gathered["resourceSpans"][0]["scopeSpans"][0]["spans"] = [
        span
        for body in [*bodies, *transport.openings]
        for span in body["resourceSpans"][0]["scopeSpans"][0]["spans"]
    ]
    assert normalize(gathered) == fixture[case["sends"]]


def test_a_trace_is_sent_from_its_start(transport: FakeTransport) -> None:
    fixture = load_fixture("trace-start.json")
    scenario = fixture["scenario"]
    spec = scenario["trace"]
    telmoni = Telmoni(
        Config(
            endpoint=ENDPOINT,
            api_key="telmoni_test_key",
            environment=scenario["environment"],
            release=scenario["release"],
        ),
        transport=transport,
        sleep=lambda _: None,
        batching=Batching(schedule_delay_s=scenario["schedule_delay_ms"] / 1000),
    )
    try:
        with telmoni.trace(
            spec["agent"],
            user_id=spec["user_id"],
            session_id=spec["session_id"],
            tags=spec["tags"],
        ):
            assert transport.wait_for_openings(1, scenario["sent_within_ms"] / 1000)
            assert not transport.requests, "nothing else before the trace ends"
            first = copy.deepcopy(transport.openings)
            assert len(first) == 1, "one body, before the trace ends"
            (start,) = first[0]["resourceSpans"][0]["scopeSpans"][0]["spans"]
            assert start["startTimeUnixNano"] == start["endTimeUnixNano"]
            parent = start.pop("parentSpanId")
            normalized = normalize(first[0])
            normalized["resourceSpans"][0]["scopeSpans"][0]["spans"][0][
                "parentSpanId"
            ] = "<span:agent>"
            assert normalized == fixture["first_export"]
        assert telmoni.flush()
    finally:
        telmoni.shutdown()
    agents = [
        s for s in transport.spans() if s["name"] == f"invoke_agent {spec['agent']}"
    ]
    assert len(agents) == 1
    (agent,) = agents
    assert agent["spanId"] == parent
    assert agent["traceId"] == start["traceId"]
    assert agent["startTimeUnixNano"] == start["startTimeUnixNano"]


def normalize_failure(span: dict[str, Any]) -> dict[str, Any]:
    """The agent's span as ``failure.json``'s README normalizes it."""
    error_type = next(
        a["value"]["stringValue"]
        for a in span["attributes"]
        if a["key"] == "error.type"
    )
    sent: dict[str, Any] = {
        key: copy.deepcopy(span[key]) for key in ("status", "events") if key in span
    }
    for event in sent.get("events", []):
        at = int(event["timeUnixNano"])
        assert int(span["startTimeUnixNano"]) <= at <= int(span["endTimeUnixNano"])
        event["timeUnixNano"] = "<time>"
        attributes = []
        for attribute in sorted(event["attributes"], key=lambda a: a["key"]):
            if attribute["key"] == "exception.stacktrace":
                assert attribute["value"]["stringValue"]
                continue
            if attribute["key"] == "exception.type":
                assert attribute["value"]["stringValue"] == error_type
                attribute["value"]["stringValue"] = "<error.type>"
            attributes.append(attribute)
        event["attributes"] = attributes
    return sent


class RateLimited(Exception):
    pass


@pytest.mark.parametrize("case", load_fixture("failure.json")["cases"])
def test_a_failure_leaves_in_full_only_under_both_locks(
    case: dict[str, Any], transport: FakeTransport
) -> None:
    fixture = load_fixture("failure.json")
    scenario = fixture["scenario"]
    mode = Answer(200, {"Telmoni-Content-Mode": case["content_mode"]}, b"{}")
    # The mode to each export: a trace's spans may leave in more than one.
    transport.answers = [mode] * 4
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k", capture_content=case["capture_content"]),
        transport=transport,
    )
    try:
        with client.trace(scenario["agent"]):
            pass
        assert client.flush()
        assert client.content_mode == case["content_mode"]
        with pytest.raises(RateLimited), client.trace(scenario["agent"]):
            event = scenario["event"]
            trace.get_current_span().add_event(event["name"], event["attributes"])
            raise RateLimited(scenario["message"])
        assert client.flush()
    finally:
        client.shutdown()
    failed = transport.spans()[-1]
    assert normalize_failure(failed) == fixture[case["sends"]]
    if case["sends"] == "kept":
        assert "exception.stacktrace" in str(failed["events"][-1])
    else:
        assert scenario["message"] not in str(transport.requests)
        assert scenario["event"]["name"] not in str(transport.requests)


@pytest.mark.parametrize("case", load_fixture("ask.json")["cases"])
def test_the_mode_is_asked_before_the_first_content_it_would_send(
    case: dict[str, Any], transport: FakeTransport
) -> None:
    fixture = load_fixture("ask.json")
    transport.answers = [
        Answer(given, {}, b"")
        if isinstance(given, int)
        else Answer(200, {"Telmoni-Content-Mode": given}, b"{}")
        for given in case["answers"]
    ]
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k", capture_content=case["capture_content"]),
        transport=transport,
    )
    messages = fixture["input_messages"] if case["content"] else None
    try:
        for _ in range(2):
            with (
                client.trace(fixture["agent"]),
                client.model_call(fixture["model"], input_messages=messages),
            ):
                pass
            assert client.flush()
    finally:
        client.shutdown()

    def spans_of(body: dict[str, Any]) -> list[dict[str, Any]]:
        return [
            span
            for resource_spans in body["resourceSpans"]
            for scope_spans in resource_spans["scopeSpans"]
            for span in scope_spans["spans"]
        ]

    bodies = transport.bodies()
    assert len(bodies) == case["requests"]
    asks = [body for body in bodies if not spans_of(body)]
    assert asks == ([fixture["body"]] if case["asked"] else [])
    if case["asked"]:
        assert bodies[0] == fixture["body"]
        assert transport.requests[0][1]["Authorization"] == "Bearer k"
    first = next(body for body in bodies if spans_of(body))
    call = next(s for s in spans_of(first) if s["name"].startswith("chat "))
    sent = any(a["key"] == "gen_ai.input.messages" for a in call["attributes"])
    assert sent == case["sent"]


def test_a_file_is_put_up_to_the_platforms_cap() -> None:
    assert contract.MEDIA_MAX_BYTES == load_fixture("media.json")["max_bytes"]


@pytest.mark.parametrize("case", load_fixture("media.json")["cases"])
def test_images_are_lifted_out_of_messages_only_under_both_locks(
    case: dict[str, Any], transport: FakeTransport
) -> None:
    fixture = load_fixture("media.json")
    scenario = fixture["scenario"]
    mode = Answer(200, {"Telmoni-Content-Mode": case["content_mode"]}, b"{}")
    transport.answers = [mode] * 3
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k", capture_content=case["capture_content"]),
        transport=transport,
    )
    try:
        a_trace(client)
        # Twice, in two exports: each file is put once.
        for _ in range(2):
            with (
                client.trace(scenario["agent"]),
                client.model_call(
                    scenario["model"],
                    provider=scenario["provider"],
                    input_messages=scenario["input_messages"],
                    system_instructions=scenario["system_instructions"],
                ) as call,
            ):
                call.answered(output_messages=scenario["output_messages"])
            assert client.flush()
    finally:
        client.shutdown()
    model_calls = [s for s in transport.spans() if s["name"].startswith("chat ")]
    assert len(model_calls) == 2
    if not case["sends"]:
        assert transport.uploads == []
        for call_span in model_calls:
            assert not set(fixture["sent"]) & {
                a["key"] for a in call_span["attributes"]
            }
        return
    uploads = sorted(transport.uploads, key=lambda upload: upload[0])
    assert [
        {
            "path": url.removeprefix(client.config.endpoint),
            "content_type": headers["Content-Type"],
            "body": base64.b64encode(body).decode(),
        }
        for url, headers, body in uploads
    ] == fixture["uploads"]
    assert all(headers["Authorization"] == "Bearer k" for _, headers, _ in uploads)
    for call_span in model_calls:
        sent = {
            a["key"]: json.loads(a["value"]["stringValue"])
            for a in call_span["attributes"]
            if a["key"] in fixture["sent"]
        }
        assert sent == fixture["sent"]


class Declined(Exception):
    pass


@pytest.mark.parametrize("case", load_fixture("mask.json")["cases"])
def test_the_mask_answers_each_content_field_only_under_both_locks(
    case: dict[str, Any], transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    fixture = load_fixture("mask.json")
    scenario = fixture["scenario"]
    handed: dict[str, str] = {}

    def mask(key: str, value: str) -> str | None:
        handed[key] = value
        rule = fixture["mask"].get(key)
        if rule is None:
            return value
        if rule.get("fail"):
            raise ValueError(f"the mask failed on {value}")
        if rule.get("drop"):
            return None
        return str(rule["replace"])

    mode = Answer(200, {"Telmoni-Content-Mode": case["content_mode"]}, b"{}")
    # The mode to each export: a trace's spans may leave in more than one.
    transport.answers = [mode] * 4
    client = Telmoni(
        Config(
            endpoint=ENDPOINT,
            api_key="k",
            capture_content=case["capture_content"],
            mask=mask,
        ),
        transport=transport,
    )
    try:
        a_trace(client)
        with (
            pytest.raises(Declined),
            client.trace(
                scenario["agent"],
                user_id=scenario["user"],
                session_id=scenario["session"],
                tags=scenario["tags"],
            ),
        ):
            with client.model_call(
                scenario["model"],
                provider=scenario["provider"],
                input_messages=scenario["input_messages"],
                system_instructions=scenario["system_instructions"],
            ) as call:
                call.answered(output_messages=scenario["output_messages"])
            event = scenario["event"]
            trace.get_current_span().add_event(event["name"], event["attributes"])
            raise Declined(scenario["message"])
        assert client.flush()
    finally:
        client.shutdown()
    spans = transport.spans()
    (call_span,) = [s for s in spans if s["name"].startswith("chat ")]
    assert transport.uploads == []
    assert "4242 " not in str(transport.requests)
    assert "4242 " not in str(warnings_in(caplog))
    sent = {
        a["key"]: json.loads(a["value"]["stringValue"])
        for a in call_span["attributes"]
        if a["key"] in fixture["handed"]
    }
    if not case["masks"]:
        assert handed == {}
        assert sent == {}
        assert normalize_failure(spans[-1]) == fixture["withheld"]
        assert warnings_in(caplog) == []
        return
    assert sent == fixture["sent"]
    assert normalize_failure(spans[-1]) == fixture["kept"]
    assert warnings_in(caplog) == fixture["warnings"]
    assert [f["kind"] for f in fields_in(caplog)] == ["mask_failed"]
    assert "exception.stacktrace" in handed
    assert sorted(set(handed) - {"exception.stacktrace"}) == fixture["handed"]
    for key, value in fixture["handed_text"].items():
        assert handed[key] == value
    # As the code gave them, blob parts and all: before any was put.
    for field, key in {
        contract.INPUT_MESSAGES: "input_messages",
        contract.OUTPUT_MESSAGES: "output_messages",
        contract.SYSTEM_INSTRUCTIONS: "system_instructions",
    }.items():
        assert json.loads(handed[field]) == scenario[key]


def test_a_mask_that_raises_is_warned_of_once_and_asked_each_export(
    transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    asked: list[str] = []

    def mask(key: str, value: str) -> str:
        asked.append(key)
        raise ValueError(value)

    transport.answers = [Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")] * 3
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k", capture_content=True, mask=mask),
        transport=transport,
    )
    try:
        a_trace(client)
        for _ in range(2):
            with (
                client.trace("support-bot"),
                client.model_call("gpt-4o", input_messages="a secret"),
            ):
                pass
            assert client.flush()
    finally:
        client.shutdown()
    assert asked == ["gen_ai.input.messages"] * 2
    assert warnings_in(caplog) == [
        "telmoni: the mask failed on gen_ai.input.messages; the span was sent without it"
    ]
    assert "a secret" not in str(transport.requests)


def test_a_mask_answering_no_string_sends_the_span_without_the_field(
    transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    transport.answers = [Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")] * 3
    client = Telmoni(
        Config(
            endpoint=ENDPOINT,
            api_key="k",
            capture_content=True,
            mask=lambda key, value: 42,  # type: ignore[arg-type,return-value]
        ),
        transport=transport,
    )
    try:
        a_trace(client)
        with (
            client.trace("support-bot"),
            client.model_call("gpt-4o", input_messages="a secret"),
        ):
            pass
        assert client.flush()
    finally:
        client.shutdown()
    assert "a secret" not in str(transport.requests)
    assert [f["kind"] for f in fields_in(caplog)] == ["mask_failed"]


def test_a_mask_answering_a_value_unchanged_keeps_its_type(
    transport: FakeTransport,
) -> None:
    transport.answers = [Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")] * 3
    client = Telmoni(
        Config(
            endpoint=ENDPOINT,
            api_key="k",
            capture_content=True,
            mask=lambda key, value: "yes" if value == "true" else value,
        ),
        transport=transport,
    )
    try:
        a_trace(client)
        with client.trace("support-bot"):
            trace.get_current_span().add_event("checked", {"count": 3, "found": True})
        assert client.flush()
    finally:
        client.shutdown()
    (event,) = transport.spans()[-1]["events"]
    assert sorted(event["attributes"], key=lambda a: a["key"]) == [
        {"key": "count", "value": {"intValue": "3"}},
        {"key": "found", "value": {"stringValue": "yes"}},
    ]


def test_a_mask_that_is_no_function_is_refused() -> None:
    with pytest.raises(TypeError, match="mask must be callable"):
        Config(endpoint=ENDPOINT, mask="redact")  # type: ignore[arg-type]


def test_each_attempt_carries_this_clock_as_it_leaves() -> None:
    """In nanoseconds, a retry its own: ingest moves a trace by it."""
    unavailable = answer_from(load_fixture("answers/unavailable.json"))
    transport = FakeTransport([unavailable])
    before = time.time_ns()
    telmoni = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k"),
        transport=transport,
        sleep=lambda _: time.sleep(0.005),
    )
    try:
        a_trace(telmoni)
    finally:
        telmoni.shutdown()
    after = time.time_ns()
    sent = [int(headers["Telmoni-Sent-At"]) for _, headers, _ in transport.requests]
    assert all(before <= at <= after for at in sent), (before, sent, after)
    (_, _, body), (_, _, again) = transport.requests[:2]
    assert body == again, "the same export, sent again"
    assert sent[1] > sent[0]


def warnings_in(caplog: pytest.LogCaptureFixture) -> list[str]:
    return [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]


def fields_in(caplog: pytest.LogCaptureFixture) -> list[dict[str, Any]]:
    """Each warning's ``telmoni_warning`` as a fixture's ``fields`` spells it."""
    fields = []
    for info in infos_in(caplog):
        given: dict[str, Any] = {"kind": info.kind}
        if info.status is not None:
            given["status"] = info.status
        if info.problem_type is not None:
            given["problem_type"] = info.problem_type
        fields.append(given)
    return fields


def a_trace(telmoni: Telmoni) -> None:
    """One empty trace, sent in an export of its own."""
    with telmoni.trace("bot"):
        pass
    assert telmoni.flush()


def test_kept(sleeps: list[float], caplog: pytest.LogCaptureFixture) -> None:
    transport = FakeTransport([answer_from(load_fixture("answers/kept.json"))])
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k"), transport=transport, sleep=sleeps.append
    )
    try:
        assert client.content_mode is None
        a_trace(client)
    finally:
        client.shutdown()
    assert warnings_in(caplog) == []
    assert client.content_mode == "off"
    assert sleeps == []


def test_dropped_is_warned_once(caplog: pytest.LogCaptureFixture) -> None:
    dropped = load_fixture("answers/dropped.json")
    transport = FakeTransport([answer_from(dropped)] * 2)
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        a_trace(client)
        a_trace(client)
    finally:
        client.shutdown()
    assert warnings_in(caplog) == [dropped["warning"]]
    assert fields_in(caplog) == [dropped["fields"]]


def test_unlisted_names_the_tag_that_would_keep_it(
    caplog: pytest.LogCaptureFixture,
) -> None:
    unlisted = load_fixture("answers/unlisted.json")
    transport = FakeTransport([answer_from(unlisted)])
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        a_trace(client)
    finally:
        client.shutdown()
    assert warnings_in(caplog) == [unlisted["warning"]]
    assert fields_in(caplog) == [unlisted["fields"]]


def test_a_tag_past_the_cardinality_cap_is_warned_with_what_keeps_it(
    caplog: pytest.LogCaptureFixture,
) -> None:
    cardinality = load_fixture("answers/cardinality.json")
    transport = FakeTransport([answer_from(cardinality)])
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        a_trace(client)
    finally:
        client.shutdown()
    assert warnings_in(caplog) == [cardinality["warning"]]
    assert fields_in(caplog) == [cardinality["fields"]]


def test_refused_is_warned_once_and_never_retried(
    caplog: pytest.LogCaptureFixture,
) -> None:
    refused = load_fixture("answers/refused.json")
    transport = FakeTransport([answer_from(refused)] * 2)
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        a_trace(client)
        a_trace(client)
    finally:
        client.shutdown()
    assert warnings_in(caplog) == [refused["warning"]]
    assert fields_in(caplog) == [refused["fields"]]
    assert len(transport.requests) == 2, "one export each trace, neither sent again"


def test_rate_limited_waits_retry_after_and_sends_the_same_bytes(
    sleeps: list[float], caplog: pytest.LogCaptureFixture
) -> None:
    transport = FakeTransport(
        [answer_from(load_fixture("answers/rate-limited.json")), kept()]
    )
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    client.exporter._sleep = sleeps.append
    try:
        a_trace(client)
    finally:
        client.shutdown()
    assert sleeps == [2.0]
    assert transport.requests[0][2] == transport.requests[1][2]
    assert warnings_in(caplog) == []


def test_a_retry_after_past_the_cap_waits_the_cap(
    sleeps: list[float], caplog: pytest.LogCaptureFixture
) -> None:
    transport = FakeTransport(
        [answer_from(load_fixture("answers/rate-limited-long.json")), kept()]
    )
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k"), transport=transport, sleep=sleeps.append
    )
    try:
        a_trace(client)
    finally:
        client.shutdown()
    assert sleeps == [60.0]
    assert transport.requests[0][2] == transport.requests[1][2]
    assert warnings_in(caplog) == []


def test_retry_after_is_read_as_text(
    sleeps: list[float], caplog: pytest.LogCaptureFixture
) -> None:
    """ASCII digits, a sign allowed: below 0 none, past 60 the cap however long."""

    def limited(retry_after: str) -> Answer:
        return Answer(429, {"Retry-After": retry_after}, b"{}")

    transport = FakeTransport(
        [
            *(limited("99999999999"), limited("1_0"), kept()),
            *(limited("-5"), limited(" +7\t"), kept()),
            *(limited("9" * 400), kept()),
        ]
    )
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k"), transport=transport, sleep=sleeps.append
    )
    try:
        for _ in range(3):
            a_trace(client)
    finally:
        client.shutdown()
    assert sleeps == [60.0, 2.0, 0.0, 7.0, 60.0]
    assert len(transport.requests) == 8
    assert warnings_in(caplog) == []


def test_unavailable_is_retried_three_times_then_dropped(
    sleeps: list[float], caplog: pytest.LogCaptureFixture
) -> None:
    fixture = load_fixture("answers/unavailable.json")
    unavailable = answer_from(fixture)
    transport = FakeTransport([unavailable, unavailable, unavailable, kept()])
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    client.exporter._sleep = sleeps.append
    try:
        a_trace(client)
        a_trace(client)
    finally:
        client.shutdown()
    assert sleeps == [1.0, 1.0]
    assert warnings_in(caplog) == [fixture["warning"]]
    assert fields_in(caplog) == [fixture["fields"]]
    assert len(transport.requests) == 4, (
        "three for the first trace, then the next trace's own export"
    )


def test_a_failure_whose_last_attempt_met_no_answer_names_no_status(
    caplog: pytest.LogCaptureFixture,
) -> None:
    """The last attempt's, as the line names it: the 503s before it are not."""
    unavailable = answer_from(load_fixture("answers/unavailable.json"))
    transport = FakeTransport([unavailable, unavailable, OSError("refused")])
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k"),
        transport=transport,
        sleep=lambda _: None,
    )
    try:
        a_trace(client)
    finally:
        client.shutdown()
    assert warnings_in(caplog) == [
        "telmoni: export failed after 3 attempts (OSError); the batch was dropped"
    ]
    assert fields_in(caplog) == [{"kind": "failed"}]


def test_forbidden_is_not_retried_and_the_next_batch_is_still_sent(
    caplog: pytest.LogCaptureFixture,
) -> None:
    fixture = load_fixture("answers/forbidden.json")
    forbidden = answer_from(fixture)
    transport = FakeTransport([forbidden, forbidden])
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        a_trace(client)
        a_trace(client)
    finally:
        client.shutdown()
    assert warnings_in(caplog) == [fixture["warning"]]
    assert fields_in(caplog) == [fixture["fields"]]
    assert len(transport.requests) == 2


@pytest.mark.parametrize(
    "name", sorted(path.name for path in (FIXTURES / "answers").glob("*.json"))
)
def test_each_answer_is_warned_word_for_word(
    name: str, caplog: pytest.LogCaptureFixture
) -> None:
    """Each answer met on every attempt logs its fixture's line, as the plugins read it."""
    fixture = load_fixture(f"answers/{name}")
    transport = FakeTransport([answer_from(fixture)] * 3)
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k"),
        transport=transport,
        sleep=lambda _: None,
    )
    try:
        a_trace(client)
    finally:
        client.shutdown()
    warning = fixture["warning"]
    assert warnings_in(caplog) == ([] if warning is None else [warning])
    fields = fixture["fields"]
    assert fields_in(caplog) == ([] if fields is None else [fields])


def test_an_export_that_raises_is_dropped_in_the_words_plugins_read(
    caplog: pytest.LogCaptureFixture,
) -> None:
    """A transport that raises loses its batch alone, said as the plugins match it."""
    transport = FakeTransport([RuntimeError("the transport broke"), kept()])
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        a_trace(client)
        a_trace(client)
    finally:
        client.shutdown()
    assert warnings_in(caplog) == [
        "telmoni: an export failed (RuntimeError); the batch was dropped"
    ]
    assert fields_in(caplog) == [{"kind": "export_error"}]
    assert len(transport.requests) == 2, "the next batch still went"


def test_a_network_failure_is_retried_with_backoff(
    sleeps: list[float], caplog: pytest.LogCaptureFixture
) -> None:
    transport = FakeTransport([OSError("refused"), OSError("refused"), kept()])
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    client.exporter._sleep = sleeps.append
    try:
        a_trace(client)
    finally:
        client.shutdown()
    assert sleeps == [1.0, 2.0]
    assert warnings_in(caplog) == []


def test_an_answer_broken_before_its_status_is_retried_like_a_refused_connection(
    sleeps: list[float], caplog: pytest.LogCaptureFixture
) -> None:
    transport = FakeTransport(
        [
            http.client.RemoteDisconnected(""),
            http.client.BadStatusLine(""),
            kept(),
        ]
    )
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    client.exporter._sleep = sleeps.append
    try:
        a_trace(client)
    finally:
        client.shutdown()
    assert sleeps == [1.0, 2.0]
    assert warnings_in(caplog) == []
