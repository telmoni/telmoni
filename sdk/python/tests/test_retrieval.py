from __future__ import annotations

import asyncio
import json
import logging
from collections.abc import Iterator
from typing import Any

import pytest

from conftest import FakeTransport, attributes_of, infos_in
from telmoni import Config, Telmoni, WarningInfo
from telmoni.exporter import Answer

DOCUMENTS = [{"id": "kb-112", "score": 0.91}, {"id": "kb-087", "score": 0.84}]


def searches(transport: FakeTransport) -> list[dict[str, Any]]:
    return [
        s
        for s in transport.spans()
        if s["name"].startswith("retrieval") or s["name"] == "search_memory"
    ]


def warnings_in(caplog: pytest.LogCaptureFixture) -> list[str]:
    return [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]


@pytest.fixture
def keeping(transport: FakeTransport) -> Iterator[Telmoni]:
    """A client whose content leaves: asked for, and the project's mode on."""
    transport.answers = [Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")] * 8
    client = Telmoni(Config(api_key="k", capture_content=True), transport=transport)
    yield client
    client.shutdown()


def test_a_retrieval_carries_the_conventions_names(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with (
        telmoni.trace("bot", user_id="user") as trace,
        telmoni.retrieval("kb-articles", query="refunds", top_k=5) as found,
    ):
        found.returned(DOCUMENTS)
    assert telmoni.flush()
    (span,) = searches(transport)
    assert span["name"] == "retrieval kb-articles" and span["kind"] == 3
    assert span["traceId"] == trace.trace_id
    assert attributes_of(span) == {
        "telmoni.agent": "bot",
        "user.id": "user",
        "gen_ai.operation.name": "retrieval",
        "gen_ai.data_source.id": "kb-articles",
        "gen_ai.retrieval.top_k": 5,
        "telmoni.retrieval.returned": 2,
    }
    assert "refunds" not in str(transport.requests)
    assert "kb-112" not in str(transport.requests)


def test_a_memory_search_goes_under_the_memory_names(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with (
        telmoni.trace("bot") as trace,
        trace.retrieval("preferences", top_k=3, memory=True) as found,
    ):
        found.returned([{"id": "mem-41"}])
    assert telmoni.flush()
    (span,) = searches(transport)
    assert span["name"] == "search_memory" and span["kind"] == 3
    assert span["traceId"] == trace.trace_id
    assert attributes_of(span) == {
        "telmoni.agent": "bot",
        "gen_ai.operation.name": "search_memory",
        "gen_ai.memory.store.id": "preferences",
        "gen_ai.retrieval.top_k": 3,
        "gen_ai.memory.record.count": 1,
    }


def test_outside_a_trace_a_retrieval_sends_nothing_with_one_warning(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    with telmoni.retrieval("kb", query="q", top_k=5) as found:
        found.returned(DOCUMENTS)
        found.returned(documents=DOCUMENTS)
        found.fail("TimeoutError")
    with telmoni.retrieval("kb", memory=True):
        pass
    assert telmoni.flush()
    assert transport.requests == []
    assert warnings_in(caplog) == [
        "telmoni: retrieval opened outside a trace; nothing is sent outside a trace"
    ]
    assert infos_in(caplog) == [WarningInfo("input")]


def test_a_blank_source_is_named_by_its_operation_alone_with_one_warning(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Unicode's White_Space alone is blank, as it is for an agent."""
    with telmoni.trace("bot"):
        for source in ("", " 　"):
            with telmoni.retrieval(source):
                pass
        with telmoni.retrieval("\x85", memory=True):
            pass
    assert telmoni.flush()
    spans = searches(transport)
    assert [s["name"] for s in spans] == ["retrieval", "retrieval", "search_memory"]
    for span in spans:
        sent = attributes_of(span)
        assert "gen_ai.data_source.id" not in sent
        assert "gen_ai.memory.store.id" not in sent
    assert warnings_in(caplog) == [
        (
            "telmoni: a retrieval needs its source; one without is sent named by "
            "its operation alone"
        )
    ]


def test_a_source_past_ingests_cap_is_cut_warned_once(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Ingest names a retrieval by its source, and refuses a name past 145."""
    with telmoni.trace("bot"):
        for source in ("😀" * 146, "😀" * 146, "😀" * 145):
            with telmoni.retrieval(source):
                pass
        with telmoni.retrieval("😀" * 146, memory=True):
            pass
    assert telmoni.flush()
    spans = searches(transport)
    assert [s["name"] for s in spans] == [f"retrieval {'😀' * 145}"] * 3 + [
        "search_memory"
    ]
    assert [attributes_of(s)["gen_ai.data_source.id"] for s in spans[:3]] == [
        "😀" * 145
    ] * 3
    assert attributes_of(spans[3])["gen_ai.memory.store.id"] == "😀" * 145
    assert warnings_in(caplog) == [
        (
            "telmoni: a retrieval's source must be up to 145 characters; "
            "it is sent cut to its first 145"
        )
    ]


def test_a_source_of_no_string_goes_as_its_text_with_one_warning(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    with telmoni.trace("bot"), telmoni.retrieval(42):  # type: ignore[arg-type]
        pass
    assert telmoni.flush()
    (span,) = searches(transport)
    assert attributes_of(span)["gen_ai.data_source.id"] == "42"
    assert warnings_in(caplog) == [
        "telmoni: a retrieval's source that is no string is sent as its text"
    ]


def test_a_top_k_is_read_as_a_token_count_is(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """A whole number from 0 within 64 bits, a fraction cut; else left off."""
    given: list[Any] = [0, 7.9, "4", -1, True, "many", 2**63, float("nan")]
    with telmoni.trace("bot"):
        for top_k in given:
            with telmoni.retrieval("kb", top_k=top_k):
                pass
    assert telmoni.flush()
    sent = [attributes_of(s).get("gen_ai.retrieval.top_k") for s in searches(transport)]
    assert sent == [0, 7, 4, None, None, None, None, None]
    assert warnings_in(caplog) == [
        (
            "telmoni: gen_ai.retrieval.top_k must be a whole number from 0 within "
            "64 bits; the retrieval is sent without it"
        )
    ]


@pytest.mark.parametrize(
    "given",
    ["kb-112", b"kb-112", {"id": "kb-112"}, (d for d in DOCUMENTS), None, 3],
    ids=["text", "bytes", "mapping", "generator", "none", "number"],
)
def test_returned_refuses_anything_but_a_list_whole(
    given: object,
    keeping: Telmoni,
    transport: FakeTransport,
    caplog: pytest.LogCaptureFixture,
) -> None:
    with keeping.trace("bot"), keeping.retrieval("kb") as found:
        found.returned(given)  # type: ignore[arg-type]
    assert keeping.flush()
    (span,) = searches(transport)
    sent = attributes_of(span)
    assert "telmoni.retrieval.returned" not in sent
    assert "gen_ai.retrieval.documents" not in sent
    assert warnings_in(caplog) == [
        (
            "telmoni: a retrieval's documents must be a list; they are neither "
            "sent nor counted"
        )
    ]
    assert infos_in(caplog) == [WarningInfo("input")]


def test_a_tuple_is_a_list(keeping: Telmoni, transport: FakeTransport) -> None:
    with keeping.trace("bot"), keeping.retrieval("kb") as found:
        found.returned(tuple(DOCUMENTS))
    assert keeping.flush()
    sent = attributes_of(searches(transport)[0])
    assert sent["telmoni.retrieval.returned"] == 2
    assert json.loads(sent["gen_ai.retrieval.documents"]) == DOCUMENTS


def test_the_last_list_wins_whole_and_a_refused_one_leaves_the_last(
    keeping: Telmoni, transport: FakeTransport
) -> None:
    more = [*DOCUMENTS, {"id": "kb-203", "score": 0.77}]
    with keeping.trace("bot"):
        with keeping.retrieval("then-refused") as found:
            found.returned(DOCUMENTS)
            found.returned("kb-112")  # type: ignore[arg-type]
        with keeping.retrieval("refused-then") as found:
            found.returned({"id": "kb-112"})  # type: ignore[arg-type]
            found.returned(DOCUMENTS)
        with keeping.retrieval("twice") as found:
            found.returned(more)
            found.returned(DOCUMENTS[:1])
    assert keeping.flush()
    sent = {
        s["name"]: (
            json.loads(attributes_of(s)["gen_ai.retrieval.documents"]),
            attributes_of(s)["telmoni.retrieval.returned"],
        )
        for s in searches(transport)
    }
    assert sent == {
        "retrieval then-refused": (DOCUMENTS, 2),
        "retrieval refused-then": (DOCUMENTS, 2),
        "retrieval twice": (DOCUMENTS[:1], 1),
    }


def test_an_empty_list_is_counted_0_and_sent_as_one(
    keeping: Telmoni, transport: FakeTransport
) -> None:
    with keeping.trace("bot"):
        with keeping.retrieval("kb") as found:
            found.returned([])
        with keeping.retrieval("prefs", memory=True) as found:
            found.returned([])
    assert keeping.flush()
    retrieval, memory = (attributes_of(s) for s in searches(transport))
    assert retrieval["telmoni.retrieval.returned"] == 0
    assert retrieval["gen_ai.retrieval.documents"] == "[]"
    assert memory["gen_ai.memory.record.count"] == 0
    assert memory["gen_ai.memory.records"] == "[]"


def test_the_query_and_documents_leave_only_under_both_locks(
    keeping: Telmoni, transport: FakeTransport
) -> None:
    with keeping.trace("bot"):
        with keeping.retrieval("kb", query="refunds") as found:
            found.returned(DOCUMENTS)
        with keeping.retrieval("prefs", query={"about": "tone"}, memory=True) as found:
            found.returned(DOCUMENTS[:1])
    assert keeping.flush()
    retrieval, memory = (attributes_of(s) for s in searches(transport))
    assert retrieval["gen_ai.retrieval.query.text"] == "refunds"
    assert json.loads(retrieval["gen_ai.retrieval.documents"]) == DOCUMENTS
    assert memory["gen_ai.memory.query.text"] == '{"about":"tone"}'
    assert json.loads(memory["gen_ai.memory.records"]) == DOCUMENTS[:1]
    assert "gen_ai.retrieval.query.text" not in memory


def test_a_mask_that_takes_documents_out_leaves_the_count(
    transport: FakeTransport,
) -> None:
    """The count is taken from the list handed, before the mask runs."""

    def mask(key: str, value: str) -> str | None:
        return "[]" if key == "gen_ai.retrieval.documents" else value

    transport.answers = [Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")] * 8
    client = Telmoni(
        Config(api_key="k", capture_content=True, mask=mask), transport=transport
    )
    try:
        with client.trace("bot"), client.retrieval("kb") as found:
            found.returned(DOCUMENTS)
        assert client.flush()
    finally:
        client.shutdown()
    sent = attributes_of(searches(transport)[0])
    assert sent["gen_ai.retrieval.documents"] == "[]"
    assert sent["telmoni.retrieval.returned"] == 2


def test_a_call_inside_a_retrieval_nests_under_it(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with (
        telmoni.trace("bot"),
        telmoni.retrieval("kb"),
        telmoni.model_call("embed-small", operation="embeddings"),
    ):
        pass
    assert telmoni.flush()
    by_name = {s["name"]: s for s in transport.spans()}
    retrieval = by_name["retrieval kb"]
    assert by_name["embeddings embed-small"]["parentSpanId"] == retrieval["spanId"]


def test_the_retrieval_decorator_and_async_with(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    @telmoni.retrieval("kb", top_k=2)
    def search(query: str) -> list[str]:
        return [query]

    async def remember() -> None:
        async with telmoni.retrieval("prefs", memory=True) as found:
            found.returned(["tone"])

    with telmoni.trace("bot"):
        assert search("refunds") == ["refunds"]
        asyncio.run(remember())
    assert telmoni.flush()
    assert sorted(s["name"] for s in searches(transport)) == [
        "retrieval kb",
        "search_memory",
    ]
