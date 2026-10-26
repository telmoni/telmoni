"""The guard: a run's caps on its steps and spend, counted in this process as
the server counts a call's cost, replayed from ``sdk/fixtures/guard.json``."""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any

import pytest

from conftest import FakeTransport, attributes_of, load_fixture
from telmoni import CapExceededError, Config, Price, Run, Telmoni, Usage

CORE_PRICES = Path(__file__).resolve().parents[3] / "crates/telemetry/model_prices.json"
CARRIED = Path(__file__).resolve().parents[1] / "src/telmoni/model_prices.json"


def client(transport: FakeTransport, prices: list[Price]) -> Telmoni:
    return Telmoni(Config(api_key="k", prices=prices), transport=transport)


def handed(fixture: dict[str, Any]) -> list[Price]:
    return [Price(**price) for price in fixture["prices"]]


def call(telmoni: Telmoni, run: Run, spec: dict[str, Any]) -> None:
    with telmoni.model_call(spec["model"], provider=spec.get("provider"), run=run) as c:
        for answer in spec.get("answers", [spec]):
            c.answered(
                response_model=answer.get("response_model"),
                usage=Usage(**answer["usage"]) if "usage" in answer else None,
            )


def perform(
    telmoni: Telmoni, run: Run, op: str | dict[str, int], calls: list[Any]
) -> None:
    if op == "step":
        with telmoni.step("step", run=run):
            pass
    elif op == "tool_call":
        with telmoni.tool_call("lookup", run=run):
            pass
    else:
        assert isinstance(op, dict)
        call(telmoni, run, calls[op["model_call"]])


def test_each_call_is_counted_as_the_server_counts_it(transport: FakeTransport) -> None:
    fixture = load_fixture("guard.json")
    telmoni = client(transport, handed(fixture))
    try:
        with telmoni.run("guarded") as run:
            for spec in fixture["calls"]:
                before = run.spent
                call(telmoni, run, spec)
                assert run.spent - before == pytest.approx(
                    spec["counted"], abs=1e-12
                ), spec["name"]
    finally:
        telmoni.shutdown()


@pytest.mark.parametrize(
    "case", load_fixture("guard.json")["runs"], ids=lambda case: case["name"]
)
def test_a_run_past_a_cap_raises_at_what_it_opens_next(
    transport: FakeTransport, case: dict[str, Any]
) -> None:
    fixture = load_fixture("guard.json")
    telmoni = client(transport, handed(fixture))
    refused = case["refused"]
    try:
        with telmoni.run(
            "guarded", max_steps=case.get("max_steps"), max_spend=case.get("max_spend")
        ) as run:
            for at, op in enumerate(case["ops"]):
                if refused is not None and at == refused["at"]:
                    with pytest.raises(CapExceededError) as raised:
                        perform(telmoni, run, op, fixture["calls"])
                    assert raised.value.cap == refused["cap"]
                    assert raised.value.limit == pytest.approx(refused["limit"])
                    assert raised.value.used == pytest.approx(
                        refused["used"], abs=1e-12
                    )
                    break
                perform(telmoni, run, op, fixture["calls"])
            assert run.spent == pytest.approx(case["spent"], abs=1e-12)
        assert telmoni.flush()
    finally:
        telmoni.shutdown()
    opened = [
        s["name"] for s in transport.spans() if s["name"] != "invoke_agent guarded"
    ]
    performed = case["ops"] if refused is None else case["ops"][: refused["at"]]
    assert len(opened) == len(performed), "a refused op opened nothing"


def test_an_escaping_refusal_fails_the_run_with_its_class(
    transport: FakeTransport,
) -> None:
    telmoni = client(transport, [])
    try:
        with (
            pytest.raises(CapExceededError, match="its step cap of 1"),
            telmoni.run("guarded", max_steps=1) as run,
        ):
            with run.step("first"):
                pass
            with run.step("second"):
                pass
        assert telmoni.flush()
    finally:
        telmoni.shutdown()
    (span,) = [s for s in transport.spans() if s["name"] == "invoke_agent guarded"]
    assert attributes_of(span)["error.type"] == "CapExceededError"
    assert span["status"] == {"code": 2}


def test_a_name_no_price_is_handed_for_is_priced_by_the_table_the_release_carries(
    transport: FakeTransport,
) -> None:
    rows = json.loads(CARRIED.read_text())["prices"]
    row = next(
        r for r in rows if (r["provider"], r["model"]) == ("openai", "gpt-4o-mini")
    )
    usage = Usage(
        input_tokens=10_000, output_tokens=2_000, cache_read_input_tokens=4_000
    )
    spent: list[float] = []
    for prices in ([], [Price(**row)]):
        telmoni = client(transport, prices)
        try:
            with telmoni.run("priced") as run:
                with run.model_call("gpt-4o-mini", provider="openai") as c:
                    c.answered(usage=usage)
                spent.append(run.spent)
        finally:
            telmoni.shutdown()
    assert spent[0] > 0
    assert spent[0] == spent[1], "the table's row, as if it were handed"


def test_a_call_answered_again_counts_what_its_span_holds(
    transport: FakeTransport,
) -> None:
    telmoni = client(
        transport, [Price(provider="p", model="m", input=1e-6, output=1e-5)]
    )
    try:
        with telmoni.run("guarded") as run:
            with run.model_call("m", provider="p") as c:
                # A stream's counts, answered as they arrive.
                c.answered(usage=Usage(input_tokens=2000))
                c.answered(usage=Usage(output_tokens=100))
            spent = run.spent
        assert telmoni.flush()
    finally:
        telmoni.shutdown()
    (span,) = [s for s in transport.spans() if s["name"] == "chat m"]
    held = attributes_of(span)
    assert spent == pytest.approx(
        held["gen_ai.usage.input_tokens"] * 1e-6
        + held["gen_ai.usage.output_tokens"] * 1e-5,
        abs=1e-12,
    )
    assert spent == pytest.approx(0.003, abs=1e-12)


def test_the_release_carries_the_price_file_whole() -> None:
    assert CARRIED.read_bytes() == CORE_PRICES.read_bytes(), "make prices writes it"


def test_a_call_with_neither_under_a_spend_cap_is_warned_of_once(
    transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    telmoni = client(transport, [])
    try:
        with telmoni.run("guarded", max_spend=1.0) as run:
            for _ in range(3):
                with run.model_call("my-model", provider="vllm") as c:
                    c.answered(usage=Usage(input_tokens=10))
        with (
            telmoni.run("unguarded") as run,
            run.model_call("other-model", provider="vllm") as c,
        ):
            c.answered(usage=Usage(input_tokens=10))
    finally:
        telmoni.shutdown()
    warned = [
        r.getMessage() for r in caplog.records if "counts nothing" in r.getMessage()
    ]
    assert warned == [
        (
            "telmoni: no price for my-model (vllm), which reported no cost; it counts "
            "nothing toward the run's spend cap"
        )
    ]


@pytest.mark.parametrize(
    ("caps", "why"),
    [
        ({"max_steps": 0}, "max_steps"),
        ({"max_steps": 1.5}, "max_steps"),
        ({"max_steps": True}, "max_steps"),
        ({"max_spend": 0}, "max_spend"),
        ({"max_spend": -1.0}, "max_spend"),
        ({"max_spend": float("inf")}, "max_spend"),
        ({"max_spend": float("nan")}, "max_spend"),
        ({"max_spend": "1"}, "max_spend"),
    ],
)
def test_a_cap_is_above_0(telmoni: Telmoni, caps: dict[str, Any], why: str) -> None:
    with pytest.raises(ValueError, match=why), telmoni.run("guarded", **caps):
        pass


def test_a_price_handed_is_a_name_and_numbers_from_0() -> None:
    for broken in (
        Price(provider="", model="m", input=1e-6, output=1e-6),
        Price(provider="p", model="", input=1e-6, output=1e-6),
        Price(provider="p", model="m", input=-1e-6, output=1e-6),
        Price(provider="p", model="m", input=1e-6, output=float("nan")),
        Price(provider="p", model="m", input=1e-6, output=1e-6, cache_read=True),
    ):
        with pytest.raises(ValueError):
            Config(prices=[broken])
    with pytest.raises(TypeError):
        Config(prices=[{"provider": "p"}])  # type: ignore[list-item]


def test_a_cost_goes_as_a_double_and_one_that_is_no_cost_is_left_off(
    telmoni: Telmoni, transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    costs: list[object] = [3, 0.0123, -0.5, float("nan"), float("inf"), True, "0.5"]
    with telmoni.run("bot"):
        for cost in costs:
            with telmoni.model_call("m") as c:
                c.answered(usage=Usage(input_tokens=1, cost=cost))  # type: ignore[arg-type]
    assert telmoni.flush()
    sent = [
        {a["key"]: a["value"] for a in s["attributes"]}.get("gen_ai.usage.cost")
        for s in transport.spans()
        if s["name"] == "chat m"
    ]
    assert sent == [
        {"doubleValue": 3.0},
        {"doubleValue": 0.0123},
        None,
        None,
        None,
        None,
        None,
    ]
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warnings == [
        (
            "telmoni: gen_ai.usage.cost must be a finite number of US dollars from 0; "
            "the call is sent without it"
        )
    ]
