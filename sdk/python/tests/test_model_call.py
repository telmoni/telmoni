from __future__ import annotations

from conftest import FakeTransport, attributes_of, kept
from telmoni import Config, Telmoni, Usage
from telmoni.exporter import Answer


def calls(transport: FakeTransport) -> list[dict[str, object]]:
    return [s for s in transport.spans() if s["name"].startswith("chat ")]


def test_a_model_call_carries_the_conventions_names(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with (
        telmoni.run("bot", conversation_id="conv") as run,
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
    # The first run's two exports are answered `off`, the second run's `on`;
    # waiting for each start inside its run keeps the exports apart, so the
    # mode each call is encoded under is the same on every machine.
    transport.answers = [kept(), kept(), on, on]
    client = Telmoni(Config(api_key="k", capture_content=True), transport=transport)
    try:
        with client.run("bot"):
            assert transport.wait_for(1)
            with client.model_call("m", input_messages="first"):
                pass
        assert client.flush()
        assert client.content_mode == "off"
        with client.run("bot"):
            assert transport.wait_for(3)
            with client.model_call("m", input_messages="second"):
                pass
        assert client.flush()
        assert client.content_mode == "on"
    finally:
        client.shutdown()
    first, second = calls(transport)
    assert "gen_ai.input.messages" not in attributes_of(first)
    assert attributes_of(second)["gen_ai.input.messages"] == "second"


def test_content_stays_home_without_capture_content_even_when_allowed(
    transport: FakeTransport,
) -> None:
    on = Answer(200, {"Telmoni-Content-Mode": "on"}, b"{}")
    transport.answers = [on] * 4
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
