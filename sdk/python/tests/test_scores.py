"""Scores: live on a handle, late through the client, and recorded, replaying
``sdk/fixtures/scores.json``."""

from __future__ import annotations

import copy
import json
import logging
import re
from datetime import datetime
from typing import Any

import pytest
from opentelemetry import trace
from opentelemetry.sdk.trace import Event, ReadableSpan
from opentelemetry.trace import SpanContext

from conftest import ENDPOINT, FakeTransport, answer_from, infos_in, load_fixture
from telmoni import Config, StepRecord, Telmoni, TraceRecord, WarningInfo, contract
from telmoni.exporter import Answer

FIXTURE = load_fixture("scores.json")
UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$")
SCORES_URL = f"{ENDPOINT}/v1/scores"


def given(value: Any, kind: str) -> Any:
    """A fixture's value as Python holds it: a number named in text, NaN or an
    infinity, as its float."""
    return float(value) if kind == "number" and isinstance(value, str) else value


def bounds(case: dict[str, Any]) -> list[float] | None:
    return [float(bound) for bound in case["range"]] if "range" in case else None


def warnings_in(caplog: pytest.LogCaptureFixture) -> list[str]:
    return [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]


def posted(transport: FakeTransport) -> list[tuple[dict[str, str], dict[str, Any]]]:
    """Each POST to ``/v1/scores``, its headers and its body."""
    return [
        (headers, json.loads(body))
        for url, headers, body in transport.requests
        if url == SCORES_URL
    ]


def exported(transport: FakeTransport) -> list[dict[str, Any]]:
    """Every span of every OTLP export but a trace's opening."""
    return [
        span
        for url, _headers, body in transport.requests
        if url != SCORES_URL
        for resource_spans in json.loads(body)["resourceSpans"]
        for scope_spans in resource_spans["scopeSpans"]
        for span in scope_spans["spans"]
        if span["name"] != "telmoni.trace.start"
    ]


def agent_span(transport: FakeTransport) -> dict[str, Any]:
    (span,) = [s for s in exported(transport) if s["name"] == "invoke_agent bot"]
    return span


def events_of(span: dict[str, Any]) -> list[dict[str, Any]]:
    """Its events, each time checked within the span, then ``<time>``, and
    attributes sorted by key."""
    events: list[dict[str, Any]] = copy.deepcopy(span.get("events", []))
    start, end = int(span["startTimeUnixNano"]), int(span["endTimeUnixNano"])
    for event in events:
        assert start <= int(event["timeUnixNano"]) <= end
        event["timeUnixNano"] = "<time>"
        event["attributes"] = sorted(event["attributes"], key=lambda a: a["key"])
    return events


def kept_of(event: dict[str, Any]) -> dict[str, Any]:
    """An event as a fixture's ``kept`` spells it, its range apart."""
    attributes = {
        a["key"]: next(iter(a["value"].values())) for a in event["attributes"]
    }
    kept: dict[str, Any] = {
        "name": attributes.pop(contract.EVALUATION_NAME),
        "version": attributes.pop(contract.SCORE_VERSION, ""),
    }
    if contract.EVALUATION_VALUE in attributes:
        kept["value"] = attributes.pop(contract.EVALUATION_VALUE)
    if contract.EVALUATION_LABEL in attributes:
        kept["label"] = attributes.pop(contract.EVALUATION_LABEL)
    return {"kept": kept, "rest": attributes}


@pytest.mark.parametrize("case", FIXTURE["scores"], ids=lambda c: c["case"])
def test_a_live_score_is_kept_or_refused_as_the_fixture_says(
    case: dict[str, Any], caplog: pytest.LogCaptureFixture
) -> None:
    transport = FakeTransport()
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        with client.trace("bot") as opened:
            opened.score(
                case["name"],
                given(case["value"], case["type"]),
                range=bounds(case),
                version=case.get("version"),
            )
        assert client.flush()
    finally:
        client.shutdown()
    events = agent_span(transport).get("events", [])
    if case.get("refused"):
        assert events == []
        (warning,) = warnings_in(caplog)
        assert warning.startswith("telmoni: a score was not sent: ")
        assert infos_in(caplog) == [WarningInfo("input")]
        return
    assert warnings_in(caplog) == []
    (event,) = events
    assert event["name"] == contract.EVALUATION_EVENT
    read = kept_of(event)
    assert read["kept"] == case["kept"]
    rest = read["rest"]
    if case["type"] == "boolean":
        assert rest.pop(contract.SCORE_TYPE) == "boolean"
    if "range" in case:
        assert [
            rest.pop(contract.SCORE_RANGE_MIN),
            rest.pop(contract.SCORE_RANGE_MAX),
        ] == case["range"]
    assert rest == {}


@pytest.mark.parametrize("case", FIXTURE["targets"], ids=lambda c: c["case"])
def test_a_late_scores_target_is_kept_or_refused_as_the_fixture_says(
    case: dict[str, Any], caplog: pytest.LogCaptureFixture
) -> None:
    transport = FakeTransport()
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        client.score(
            "relevance",
            0.5,
            trace_id=case.get("trace_id"),
            span_id=case.get("span_id"),
            session_id=case.get("session_id"),
        )
        assert client.flush()
    finally:
        client.shutdown()
    if case.get("refused"):
        assert transport.requests == []
        assert warnings_in(caplog) == [
            (
                "telmoni: a score was not sent: it names a trace by 32 hex digits, "
                "a trace and one of its spans by 16, or a session of up to 200 "
                "characters"
            )
        ]
        return
    assert warnings_in(caplog) == []
    ((_headers, body),) = posted(transport)
    (item,) = body["scores"]
    ids = {
        key: item[key] for key in ("trace_id", "span_id", "session_id") if key in item
    }
    assert ids == case["kept"]


def test_late_scores_leave_in_one_post_as_the_fixture_says(
    caplog: pytest.LogCaptureFixture,
) -> None:
    transport = FakeTransport()
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        for each in FIXTURE["late"]["scores"]:
            client.score(
                each["name"],
                given(each["value"], each["type"]),
                range=bounds(each),
                version=each.get("version"),
                **each["target"],
            )
        assert client.flush()
    finally:
        client.shutdown()
    assert warnings_in(caplog) == []
    assert [url for url, _h, _b in transport.requests] == [SCORES_URL], (
        "one POST, and no export of the carrier spans"
    )
    ((headers, body),) = posted(transport)
    for item in body["scores"]:
        assert UUID.match(item["id"])
        item["id"] = "<uuid>"
    assert body == FIXTURE["late"]["sent"]
    assert headers["Content-Type"] == "application/json"
    assert headers["Authorization"] == "Bearer k"
    assert headers["User-Agent"].startswith("telmoni-python/")


def test_live_scores_are_the_fixtures_events_without_content() -> None:
    transport = FakeTransport()
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        with client.trace("bot") as opened:
            for each in FIXTURE["live"]["scores"]:
                opened.score(
                    each["name"],
                    given(each["value"], each["type"]),
                    range=bounds(each),
                    version=each.get("version"),
                )
        assert client.flush()
    finally:
        client.shutdown()
    assert events_of(agent_span(transport)) == FIXTURE["live"]["events"]


def test_an_evaluation_events_other_attributes_are_content() -> None:
    """The application's own evaluation event leaves with a score's keys alone,
    and with the rest only under both locks."""
    transport = FakeTransport()
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        with client.trace("bot"):
            trace.get_current_span().add_event(
                contract.EVALUATION_EVENT,
                {
                    contract.EVALUATION_NAME: "relevance",
                    contract.EVALUATION_VALUE: 0.5,
                    "gen_ai.evaluation.explanation": "4242 cites the source",
                },
            )
        assert client.flush()
    finally:
        client.shutdown()
    (event,) = agent_span(transport)["events"]
    assert {a["key"] for a in event["attributes"]} == {
        contract.EVALUATION_NAME,
        contract.EVALUATION_VALUE,
    }
    # With its space: the bare digits can fall in an id or a time by chance.
    assert "4242 " not in str(transport.requests)


def test_a_refused_post_is_warned_once_and_never_retried(
    caplog: pytest.LogCaptureFixture,
) -> None:
    problem = b'{"message": "/errors/bad-request: scores[0]: no"}'
    transport = FakeTransport([Answer(400, {}, problem)])
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k"),
        transport=transport,
        sleep=lambda _: None,
    )
    try:
        client.score("thumbs", True, session_id="conv-1")
        assert client.flush()
    finally:
        client.shutdown()
    assert len(transport.requests) == 1
    assert warnings_in(caplog) == [
        "telmoni: scores refused with 400 /errors/bad-request; they were dropped"
    ]
    assert infos_in(caplog) == [WarningInfo("refused", 400, "/errors/bad-request")]


def test_an_unavailable_post_is_retried_and_sent_once_more(
    sleeps: list[float], caplog: pytest.LogCaptureFixture
) -> None:
    unavailable = answer_from(load_fixture("answers/unavailable.json"))
    transport = FakeTransport([unavailable, Answer(204, {}, b"")])
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k"), transport=transport, sleep=sleeps.append
    )
    try:
        client.score("thumbs", True, session_id="conv-1")
        assert client.flush()
    finally:
        client.shutdown()
    (first, second) = transport.requests
    assert first[0] == second[0] == SCORES_URL
    assert first[2] == second[2], "the same body, its id the same"
    assert sleeps == [1.0]
    assert warnings_in(caplog) == []


def test_a_post_failing_every_attempt_is_dropped_in_its_own_words(
    caplog: pytest.LogCaptureFixture,
) -> None:
    unavailable = answer_from(load_fixture("answers/unavailable.json"))
    transport = FakeTransport([unavailable] * 3)
    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k"),
        transport=transport,
        sleep=lambda _: None,
    )
    try:
        client.score("thumbs", True, session_id="conv-1")
        assert client.flush()
    finally:
        client.shutdown()
    assert len(transport.requests) == 3
    assert warnings_in(caplog) == [
        (
            "telmoni: scores failed after 3 attempts "
            "(503 /errors/telemetry/store-unavailable); they were dropped"
        )
    ]
    assert infos_in(caplog) == [
        WarningInfo("failed", 503, "/errors/telemetry/store-unavailable")
    ]


def test_scores_past_a_bodys_cap_go_in_bodies_of_100_in_order() -> None:
    transport = FakeTransport()
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        for at in range(101):
            client.score(f"s{at}", at, session_id="conv-1")
        assert client.flush()
    finally:
        client.shutdown()
    names = [
        [item["name"] for item in body["scores"]] for _h, body in posted(transport)
    ]
    assert [len(each) for each in names] == [100, 1]
    assert [name for each in names for name in each] == [f"s{at}" for at in range(101)]


def test_late_scores_beside_spans_leave_apart_from_them() -> None:
    transport = FakeTransport()
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        with client.trace("bot") as opened:
            client.score(
                "thumbs", False, trace_id=opened.trace_id, span_id=opened.span_id
            )
        assert client.flush()
    finally:
        client.shutdown()
    span = agent_span(transport)
    assert [s["name"] for s in exported(transport)] == ["invoke_agent bot"]
    ((_headers, body),) = posted(transport)
    (item,) = body["scores"]
    assert (item["trace_id"], item["span_id"]) == (opened.trace_id, span["spanId"])
    assert span["traceId"] == opened.trace_id


def test_a_score_past_its_spans_cap_or_its_end_is_not_sent(
    caplog: pytest.LogCaptureFixture,
) -> None:
    most = FIXTURE["limits"]["per_span_max"]
    transport = FakeTransport()
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        with client.trace("bot") as opened:
            for at in range(most + 1):
                opened.score(f"s{at}", 0.5)
            with client.step("plan") as step:
                pass
            step.score("late", 0.5)
            # Refused ones take no room: the step's first score was refused.
        assert client.flush()
    finally:
        client.shutdown()
    assert len(agent_span(transport)["events"]) == most
    (step_span,) = [s for s in exported(transport) if s["name"] == "plan"]
    assert "events" not in step_span
    assert warnings_in(caplog) == [
        f"telmoni: a score was not sent: its span holds {most} scores already",
        (
            "telmoni: a score was not sent: its span had ended; send it with the "
            "client's score, naming the span"
        ),
    ]


def test_a_score_of_the_wrong_type_or_a_range_off_a_number_is_refused(
    caplog: pytest.LogCaptureFixture,
) -> None:
    transport = FakeTransport()
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        with client.trace("bot") as opened:
            opened.score("relevance", None)  # type: ignore[arg-type]
            opened.score("relevance", [0.5])  # type: ignore[arg-type]
            opened.score("tone", "polite", range=[0, 1])
            opened.score("grounded", True, range=[0, 1])
            opened.score("relevance", 10**400)
        assert client.flush()
    finally:
        client.shutdown()
    assert "events" not in agent_span(transport)
    assert warnings_in(caplog) == [
        (
            "telmoni: a score was not sent: its value is a number, a yes or no, "
            "or a category"
        ),
        "telmoni: a score was not sent: a range is a number score's alone",
        (
            "telmoni: a score was not sent: a number score is a finite number "
            "within its range"
        ),
    ]


def test_a_handle_names_its_spans_id() -> None:
    transport = FakeTransport()
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        with client.trace("bot") as opened:
            pass
        assert client.flush()
    finally:
        client.shutdown()
    assert re.fullmatch("[0-9a-f]{16}", opened.span_id)
    assert opened.span_id == agent_span(transport)["spanId"]


def test_a_recorded_score_is_timed_at_its_spans_end(
    caplog: pytest.LogCaptureFixture,
) -> None:
    def at(text: str) -> datetime:
        return datetime.fromisoformat(text)

    record = TraceRecord(
        trace_id="5b7e1c2a9d4f4e8ba6c31f2e3d4c5b6a",
        agent="bot",
        start=at("2026-10-01T10:00:00+00:00"),
        end=at("2026-10-01T10:00:05+00:00"),
        scores=[
            {"name": "relevance", "value": 0.5, "range": [0, 1], "version": "j"},
            {"name": "relevance", "value": 2, "range": [0, 1]},
        ],
        inside=[
            StepRecord(
                name="plan",
                start=at("2026-10-01T10:00:01+00:00"),
                end=at("2026-10-01T10:00:02+00:00"),
                scores=[{"name": "grounded", "value": True}],
            )
        ],
    )
    sent: list[bytes] = []
    for _ in range(2):
        transport = FakeTransport()
        client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
        try:
            client.record_trace(record)
            assert client.flush()
        finally:
            client.shutdown()
        sent.append(transport.requests[0][2])
    assert sent[0] == sent[1], "the same record sent again is the same bytes"
    spans = {s["name"]: s for s in exported(transport)}
    for name, scored in (("invoke_agent bot", "relevance"), ("plan", "grounded")):
        span = spans[name]
        (event,) = span["events"]
        assert event["timeUnixNano"] == span["endTimeUnixNano"]
        assert kept_of(event)["kept"]["name"] == scored
    assert (
        warnings_in(caplog)
        == [
            (
                "telmoni: a score was not sent: a number score is a finite number "
                "within its range"
            )
        ]
        * 2
    ), "once a client, the record still sent"


def test_a_recorded_score_that_is_no_mapping_refuses_the_record() -> None:
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=FakeTransport())
    try:
        with pytest.raises(TypeError):
            client.record_trace(
                TraceRecord(
                    trace_id="5b7e1c2a9d4f4e8ba6c31f2e3d4c5b6a",
                    agent="bot",
                    start=datetime.fromisoformat("2026-10-01T10:00:00+00:00"),
                    end=datetime.fromisoformat("2026-10-01T10:00:05+00:00"),
                    scores=[("relevance", 0.5)],  # type: ignore[list-item]
                )
            )
    finally:
        client.shutdown()


def test_the_limits_are_the_fixtures() -> None:
    assert FIXTURE["limits"] == {
        "name_max": contract.SCORE_NAME_MAX,
        "category_max": contract.SCORE_CATEGORY_MAX,
        "version_max": contract.SCORE_VERSION_MAX,
        "session_max": contract.SESSION_ID_MAX,
        "per_span_max": contract.SCORES_PER_SPAN_MAX,
        "sent_max": contract.SCORES_SENT_MAX,
    }


def test_a_score_event_leaves_untouched_by_a_mask_dropping_everything() -> None:
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    transport = FakeTransport([on, on])
    handed: list[str] = []

    def mask(key: str, value: str) -> None:
        handed.append(key)

    client = Telmoni(
        Config(endpoint=ENDPOINT, api_key="k", capture_content=True, mask=mask),
        transport=transport,
    )
    try:
        with client.trace("warm"):
            pass
        assert client.flush()
        with client.trace("bot") as opened:
            score = FIXTURE["live"]["scores"][0]
            opened.score(
                score["name"],
                score["value"],
                range=score["range"],
                version=score["version"],
            )
            trace.get_current_span().add_event("cache miss", {"key": "k1"})
        assert client.flush()
    finally:
        client.shutdown()
    scored, missed = events_of(agent_span(transport))
    assert scored == FIXTURE["live"]["events"][0]
    assert missed["attributes"] == [], "the mask dropped the other event's"
    assert handed == ["key"]


def test_a_late_scores_trace_in_capitals_is_sent_lowercase() -> None:
    """Read as ``record_trace`` reads a trace's id."""
    transport = FakeTransport()
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    try:
        client.score("thumbs", True, trace_id="5B7E1C2A9D4F4E8BA6C31F2E3D4C5B6A")
        assert client.flush()
    finally:
        client.shutdown()
    ((_headers, body),) = posted(transport)
    assert body["scores"][0]["trace_id"] == "5b7e1c2a9d4f4e8ba6c31f2e3d4c5b6a"


def test_a_span_trimmed_to_fit_keeps_its_scores_keys(
    caplog: pytest.LogCaptureFixture,
) -> None:
    transport = FakeTransport()
    client = Telmoni(Config(endpoint=ENDPOINT, api_key="k"), transport=transport)
    big = ReadableSpan(
        name="big",
        context=SpanContext(trace_id=1, span_id=1, is_remote=False),
        start_time=1,
        end_time=2,
        attributes={"telmoni.tag.big": "x" * (1 << 20)},
        events=[
            Event(
                contract.EVALUATION_EVENT,
                {contract.EVALUATION_NAME: "relevance", contract.EVALUATION_VALUE: 0.5},
                2,
            )
        ],
    )
    try:
        client.exporter.export([big])
    finally:
        client.shutdown()
    (span,) = exported(transport)
    assert "telmoni.tag.big" not in str(span["attributes"])
    assert span["events"][0]["attributes"] == [
        {"key": contract.EVALUATION_NAME, "value": {"stringValue": "relevance"}},
        {"key": contract.EVALUATION_VALUE, "value": {"doubleValue": 0.5}},
    ]
    assert infos_in(caplog) == [WarningInfo("trimmed")]
