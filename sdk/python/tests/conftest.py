from __future__ import annotations

import json
import threading
from collections.abc import Iterator, Mapping
from pathlib import Path
from typing import Any

import pytest
from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import (
    InMemorySpanExporter,
)
from opentelemetry.sdk.trace.sampling import (
    ALWAYS_ON,
    ParentBased,
    Sampler,
    SamplingResult,
)

from telmoni import Config, Telmoni
from telmoni.exporter import Answer

FIXTURES = Path(__file__).resolve().parents[2] / "fixtures"


def load_fixture(name: str) -> Any:
    return json.loads((FIXTURES / name).read_text())


def answer_from(fixture: Mapping[str, Any]) -> Answer:
    return Answer(
        int(fixture["status"]),
        dict(fixture["headers"]),
        json.dumps(fixture["body"]).encode(),
    )


def kept() -> Answer:
    return answer_from(load_fixture("answers/kept.json"))


class FakeTransport:
    """Records every export and answers from a script, then with `kept`."""

    def __init__(self, answers: list[Answer | Exception] | None = None) -> None:
        self.requests: list[tuple[str, dict[str, str], bytes]] = []
        self.answers = list(answers or [])
        self._lock = threading.Lock()
        self._received = threading.Condition(self._lock)

    def __call__(self, url: str, headers: Mapping[str, str], body: bytes) -> Answer:
        with self._lock:
            self.requests.append((url, dict(headers), body))
            answer = self.answers.pop(0) if self.answers else kept()
            self._received.notify_all()
        if isinstance(answer, Exception):
            raise answer
        return answer

    def wait_for(self, count: int, timeout: float = 5.0) -> bool:
        with self._lock:
            return self._received.wait_for(lambda: len(self.requests) >= count, timeout)

    def bodies(self) -> list[dict[str, Any]]:
        with self._lock:
            return [json.loads(body) for _, _, body in self.requests]

    def spans(self) -> list[dict[str, Any]]:
        return [
            span
            for body in self.bodies()
            for resource_spans in body["resourceSpans"]
            for scope_spans in resource_spans["scopeSpans"]
            for span in scope_spans["spans"]
        ]


def attributes_of(span: Mapping[str, Any]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for attribute in span["attributes"]:
        value = attribute["value"]
        if "intValue" in value:
            out[attribute["key"]] = int(value["intValue"])
        else:
            out[attribute["key"]] = next(iter(value.values()))
    return out


class SwitchableSampler(Sampler):
    """The application's sampler in the tests: its target is swapped per test."""

    def __init__(self) -> None:
        self.target: Sampler = ALWAYS_ON

    def should_sample(self, *args: Any, **kwargs: Any) -> SamplingResult:
        return self.target.should_sample(*args, **kwargs)

    def get_description(self) -> str:
        return f"Switchable({self.target.get_description()})"


APP_SAMPLER = SwitchableSampler()
APP_EXPORTER = InMemorySpanExporter()
# The global provider can be set once per process: this is the application's
# own OpenTelemetry, which every test here runs beside.
# ParentBased, as an application's default sampler is: its root decision is the
# switchable target's, and a span inside a run follows the run's flag.
APP_PROVIDER = TracerProvider(sampler=ParentBased(APP_SAMPLER))
APP_PROVIDER.add_span_processor(SimpleSpanProcessor(APP_EXPORTER))
trace.set_tracer_provider(APP_PROVIDER)


@pytest.fixture
def app_sampler() -> Iterator[SwitchableSampler]:
    yield APP_SAMPLER
    APP_SAMPLER.target = ALWAYS_ON


@pytest.fixture
def app_exporter() -> Iterator[InMemorySpanExporter]:
    APP_EXPORTER.clear()
    yield APP_EXPORTER
    APP_EXPORTER.clear()


@pytest.fixture
def transport() -> FakeTransport:
    return FakeTransport()


@pytest.fixture
def sleeps() -> list[float]:
    return []


@pytest.fixture
def telmoni(transport: FakeTransport, sleeps: list[float]) -> Iterator[Telmoni]:
    client = Telmoni(
        Config(api_key="telmoni_test_key"), transport=transport, sleep=sleeps.append
    )
    yield client
    client.shutdown()
