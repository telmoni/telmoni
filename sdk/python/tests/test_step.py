from __future__ import annotations

from conftest import FakeTransport, attributes_of
from telmoni import Telmoni


def by_name(transport: FakeTransport) -> dict[str, dict[str, object]]:
    return {str(s["name"]): s for s in transport.spans()}


def test_a_step_nests_under_its_run_and_carries_its_ids(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with (
        telmoni.run("bot", user_id="c", conversation_id="v") as run,
        telmoni.step("plan"),
        telmoni.step("plan.inner"),
    ):
        pass
    assert telmoni.flush()
    spans = by_name(transport)
    plan, inner, agent = spans["plan"], spans["plan.inner"], spans["invoke_agent bot"]
    assert plan["parentSpanId"] == agent["spanId"]
    assert inner["parentSpanId"] == plan["spanId"]
    assert plan["traceId"] == agent["traceId"] == inner["traceId"]
    assert attributes_of(plan) == {
        "telmoni.run_id": run.run_id,
        "telmoni.agent": "bot",
        "user.id": "c",
        "gen_ai.conversation.id": "v",
    }
    assert plan["kind"] == 1 and "status" not in plan


def test_a_step_opened_on_the_run_handle_is_the_same(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    with telmoni.run("bot") as run, run.step("explicit"):
        pass
    assert telmoni.flush()
    step = by_name(transport)["explicit"]
    assert attributes_of(step)["telmoni.run_id"] == run.run_id


def test_a_failing_step_records_the_error_class_and_reraises(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    class PlanningFailed(RuntimeError):
        pass

    with telmoni.run("bot"):
        try:
            with telmoni.step("plan"):
                raise PlanningFailed("secret details")
        except PlanningFailed:
            pass
    assert telmoni.flush()
    spans = by_name(transport)
    assert spans["plan"]["status"] == {"code": 2}
    assert attributes_of(spans["plan"])["error.type"] == "PlanningFailed"
    assert "status" not in spans["invoke_agent bot"], "the run itself went on"
    assert "secret details" not in str(transport.requests)


def test_the_step_decorator(telmoni: Telmoni, transport: FakeTransport) -> None:
    @telmoni.step("plan")
    def plan(n: int) -> int:
        return n + 1

    with telmoni.run("bot"):
        assert plan(1) == 2
    assert telmoni.flush()
    assert "plan" in by_name(transport)
