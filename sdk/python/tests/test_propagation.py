"""A run across services: ``inject`` writes the W3C ``traceparent`` and our
two ``baggage`` members as ``sdk/fixtures/headers.json`` spells them,
``extract`` reads them back as a run continued here, and nothing of a
continued run but its steps and calls is sent."""

from __future__ import annotations

from typing import Any

from opentelemetry import trace

from conftest import FakeTransport, attributes_of, load_fixture
from telmoni import Config, Run, Telmoni, current_run
from telmoni.propagation import (
    decode_baggage_value,
    encode_baggage_value,
    parse_traceparent,
)

FIXTURE: dict[str, Any] = load_fixture("headers.json")


def by_name(transport: FakeTransport) -> dict[str, dict[str, Any]]:
    return {span["name"]: span for span in transport.spans()}


def test_inject_writes_the_headers_the_fixture_spells_beside_the_carriers_own(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    spec = FIXTURE["inject"]
    headers: dict[str, str] = {}
    beside: dict[str, str] = {"Baggage": spec["beside"]["given"]}
    with telmoni.run(spec["agent"]) as run, telmoni.step("plan"):
        telmoni.inject(headers)
        telmoni.inject(beside)
    assert telmoni.flush()
    step = by_name(transport)["plan"]

    def fill(template: str) -> str:
        return (
            template.replace("<trace>", step["traceId"])
            .replace("<span>", step["spanId"])
            .replace("<uuid>", run.run_id)
        )

    assert headers == {
        "traceparent": fill(spec["traceparent"]),
        "baggage": fill(spec["baggage"]),
    }
    assert beside == {
        "traceparent": fill(spec["traceparent"]),
        "baggage": fill(spec["beside"]["written"]),
    }, "the carrier's members kept, ours replaced, the header's key set once"


def test_inject_writes_nothing_outside_a_run_or_for_a_run_sampled_out(
    transport: FakeTransport,
) -> None:
    client = Telmoni(
        Config(api_key="k", sample_rate=0.5), transport=transport, sleep=lambda _: None
    )
    try:
        outside = {"baggage": "tenant=acme"}
        client.inject(outside)
        assert outside == {"baggage": "tenant=acme"}
        from unittest import mock

        with (
            mock.patch("telmoni.runs.random.random", return_value=0.9),
            client.run("bot") as run,
        ):
            assert run.sampled_out
            headers: dict[str, str] = {}
            client.inject(headers)
            assert headers == {}
        assert client.flush()
    finally:
        client.shutdown()
    assert transport.spans() == []


def test_extract_reads_the_run_the_fixture_names_and_nothing_from_headers_naming_none(
    telmoni: Telmoni,
) -> None:
    for given in FIXTURE["extract"]:
        run = telmoni.extract(given["headers"])
        if given["run"] is None:
            assert run is None, given["name"]
            continue
        assert isinstance(run, Run), given["name"]
        assert run.run_id == given["run"]["run_id"], given["name"]
        assert run.agent == given["run"]["agent"], given["name"]
        assert not run.sampled_out, given["name"]
        parent = run._span.get_span_context()
        assert f"{parent.trace_id:032x}" == given["run"]["trace_id"], given["name"]
        assert f"{parent.span_id:016x}" == given["run"]["parent_span_id"], given["name"]
        assert parent.is_remote, given["name"]
        assert bool(parent.trace_flags.sampled) is given["run"]["sampled"], given[
            "name"
        ]


def test_a_continued_runs_steps_and_calls_carry_the_callers_run_under_its_span(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    headers: dict[str, str] = {}
    with telmoni.run("orchestrator") as run, telmoni.step("hand over"):
        telmoni.inject(headers)
    assert telmoni.flush()
    caller = by_name(transport)["hand over"]

    worker_transport = FakeTransport()
    worker = Telmoni(
        Config(api_key="k"), transport=worker_transport, sleep=lambda _: None
    )
    try:
        continued = worker.extract(headers)
        assert isinstance(continued, Run)
        with worker.step("work", run=continued):
            assert current_run() is continued
            with worker.model_call("gpt-x", provider="openai"):
                pass
        with continued.tool_call("search"):
            pass
        continued.fail("Ignored")
        assert worker.flush()
    finally:
        worker.shutdown()
    spans = worker_transport.spans()
    assert sorted(span["name"] for span in spans) == [
        "chat gpt-x",
        "execute_tool search",
        "work",
    ], "nothing of the run itself: no invoke_agent, no telmoni.run.start"
    for span in spans:
        assert span["traceId"] == caller["traceId"], span["name"]
        attributes = attributes_of(span)
        assert attributes["telmoni.run_id"] == run.run_id, span["name"]
        assert attributes["telmoni.agent"] == "orchestrator", span["name"]
    named = by_name(worker_transport)
    assert named["work"]["parentSpanId"] == caller["spanId"]
    assert named["execute_tool search"]["parentSpanId"] == caller["spanId"]
    assert named["chat gpt-x"]["parentSpanId"] == named["work"]["spanId"]


def test_a_continued_runs_spans_carry_its_agent_cut_to_its_cap(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    """Uncut, ingest would refuse every span this side sends of the run."""
    given = next(
        case for case in FIXTURE["extract"] if case["name"].startswith("an agent past")
    )
    continued = telmoni.extract(given["headers"])
    assert isinstance(continued, Run)
    with telmoni.step("work", run=continued):
        pass
    assert telmoni.flush()
    assert (
        attributes_of(by_name(transport)["work"])["telmoni.agent"]
        == (given["run"]["agent"])
    )


def test_a_continued_run_hands_itself_on_again_under_the_span_current_here(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    first = {
        "traceparent": "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        "baggage": "telmoni.run_id=0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01,telmoni.agent=bot",
    }
    onwards: dict[str, str] = {}
    continued = telmoni.extract(first)
    assert continued is not None
    with continued.step("relay"):
        telmoni.inject(onwards)
    assert telmoni.flush()
    relay = by_name(transport)["relay"]
    assert onwards == {
        "traceparent": f"00-4bf92f3577b34da6a3ce929d0e0e4736-{relay['spanId']}-01",
        "baggage": "telmoni.run_id=0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01,telmoni.agent=bot",
    }
    assert trace.get_current_span().get_span_context().is_valid is False


def test_a_baggage_value_is_encoded_as_the_grammar_asks_and_read_back() -> None:
    value = 'support bot, "the" one; é\\%'
    assert (
        encode_baggage_value(value)
        == "support%20bot%2C%20%22the%22%20one%3B%20%C3%A9%5C%25"
    )
    assert decode_baggage_value(encode_baggage_value(value)) == value
    assert decode_baggage_value("a%2") == "a%2"
    assert decode_baggage_value("a%zz") == "a%zz"
    assert decode_baggage_value("a%0\n") == "a%0\n"
    assert decode_baggage_value("%c3%A9") == "é"
    plain = "plain-bot_1.0~!#$&'()*+/:<=>?@[]^`{|}"
    assert encode_baggage_value(plain) == plain


def test_a_baggage_value_with_a_lone_surrogate_goes_with_the_replacement_character() -> (
    None
):
    """Encoded strictly, it would raise into the agent's call that injects it."""
    assert encode_baggage_value("bot\udcff") == "bot%EF%BF%BD"


def test_a_traceparent_is_parsed_as_the_grammar_asks() -> None:
    parsed = parse_traceparent(
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
    )
    assert parsed is not None
    assert f"{parsed.trace_id:032x}" == "4bf92f3577b34da6a3ce929d0e0e4736"
    assert f"{parsed.span_id:016x}" == "00f067aa0ba902b7"
    assert parsed.trace_flags.sampled and parsed.is_remote
    flagged = parse_traceparent(
        " 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-03 "
    )
    assert flagged is not None and flagged.trace_flags == 1
    future = parse_traceparent(
        "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00-more"
    )
    assert future is not None and not future.trace_flags.sampled
    for refused in [
        "",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
        "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        "00-4bf92f3577b34da6a3ce929d0e0e473-00f067aa0ba902b7-01",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b-01",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-1",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-0g",
    ]:
        assert parse_traceparent(refused) is None, refused
