from __future__ import annotations

import logging

import pytest

from conftest import FakeTransport, attributes_of
from telmoni import Telmoni


def tools(transport: FakeTransport) -> list[dict[str, object]]:
    return [s for s in transport.spans() if s["name"].startswith("execute_tool ")]


def test_a_tool_call_carries_the_conventions_names(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with (
        telmoni.run("bot", user_id="user") as run,
        telmoni.tool_call("search", arguments={"q": "weather"}) as tool,
    ):
        tool.returned({"temp": 21})
    assert telmoni.flush()
    (span,) = tools(transport)
    assert span["name"] == "execute_tool search" and span["kind"] == 1
    assert attributes_of(span) == {
        "telmoni.run_id": run.run_id,
        "telmoni.agent": "bot",
        "user.id": "user",
        "gen_ai.operation.name": "execute_tool",
        "gen_ai.tool.name": "search",
    }
    assert "weather" not in str(transport.requests) and "temp" not in str(
        transport.requests
    )


def test_a_failing_tool_records_its_error_class(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.run("bot"):
        try:
            with telmoni.tool_call("search"):
                raise TimeoutError("took 30 s")
        except TimeoutError:
            pass
        with telmoni.tool_call("lookup") as tool:
            tool.fail("NotFound")
    assert telmoni.flush()
    by_name = {s["name"]: s for s in tools(transport)}
    assert attributes_of(by_name["execute_tool search"])["error.type"] == "TimeoutError"
    assert by_name["execute_tool search"]["status"] == {"code": 2}
    assert attributes_of(by_name["execute_tool lookup"])["error.type"] == "NotFound"


def test_a_tool_name_past_ingests_cap_is_cut_warned_once(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """Ingest refuses a tool call named past 145 characters."""
    with telmoni.run("bot"):
        for name in ("t" * 146, "t" * 146, "t" * 145):
            with telmoni.tool_call(name):
                pass
    assert telmoni.flush()
    assert [attributes_of(s)["gen_ai.tool.name"] for s in tools(transport)] == [
        "t" * 145
    ] * 3
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warnings == [
        (
            "telmoni: a tool's name must be up to 145 characters; "
            "it is sent cut to its first 145"
        )
    ]


def test_the_tool_call_decorator(telmoni: Telmoni, transport: FakeTransport) -> None:
    @telmoni.tool_call("search")
    def search(q: str) -> str:
        return q[::-1]

    with telmoni.run("bot"):
        assert search("abc") == "cba"
    assert telmoni.flush()
    assert len(tools(transport)) == 1


def test_a_scope_entered_again_while_open_refuses_and_keeps_its_span(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.run("bot"):
        search = telmoni.tool_call("search")
        with search, pytest.raises(RuntimeError, match="this scope is open"):
            search.__enter__()
        # Entered again once it has closed, it opens another.
        with search:
            pass
    assert telmoni.flush()
    assert len(tools(transport)) == 2


def test_content_that_will_not_serialize_never_raises(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    cyclic: dict[str, object] = {}
    cyclic["self"] = cyclic
    with telmoni.run("bot"), telmoni.tool_call("search", arguments=cyclic) as tool:
        tool.returned(cyclic)
    assert telmoni.flush()
    (span,) = tools(transport)
    assert span["name"] == "execute_tool search"
