"""Scores: a name and a value, of a run, one of its spans or a session.

A live score is a ``gen_ai.evaluation.result`` event on its handle's span; a
late one, of what has ended, travels the queue as a ``telmoni.score`` span,
which the exporter takes out and posts to ``/v1/scores``. One breaking a
score's rules is never raised: it is not sent, with one warning.
"""

from __future__ import annotations

import json
import math
import re
import uuid
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from . import contract, ids
from .models import WarningInfo, _unset_if_blank

Warn = Callable[[str, str, WarningInfo], None]

NAME = "its name is 1 to 64 characters"
NUMBER = "a number score is a finite number within its range"
CATEGORY = "its category is 1 to 64 characters"
VERSION = "its version is up to 64 characters"
RANGE = "a range is a number score's alone"
VALUE = "its value is a number, a yes or no, or a category"
TARGET = (
    "it names a run, a run and one of its spans by 16 hex digits, "
    "or a session of up to 200 characters"
)
ENDED = "its span had ended; send it with the client's score, naming the span"
FULL = "its span holds 16 scores already"

_SPAN_ID = re.compile(r"[0-9a-fA-F]{16}")


@dataclass(frozen=True)
class Judgement:
    """A score held to its rules: trimmed, its number a float."""

    name: str
    data_type: str
    value: float | None = None
    label: str = ""
    range: tuple[float, float] | None = None
    version: str = ""

    def event(self) -> dict[str, Any]:
        """The evaluation event's attributes."""
        attributes: dict[str, Any] = {contract.EVALUATION_NAME: self.name}
        if self.value is not None:
            attributes[contract.EVALUATION_VALUE] = self.value
        if self.data_type == "boolean":
            attributes[contract.SCORE_TYPE] = "boolean"
        elif self.data_type == "category":
            attributes[contract.EVALUATION_LABEL] = self.label
        if self.range is not None:
            attributes[contract.SCORE_RANGE_MIN] = self.range[0]
            attributes[contract.SCORE_RANGE_MAX] = self.range[1]
        if self.version:
            attributes[contract.SCORE_VERSION] = self.version
        return attributes

    def sent(self, target: dict[str, str]) -> str:
        """``POST /v1/scores``'s item as compact JSON, its id fresh, so a retry
        replaces itself."""
        item: dict[str, Any] = {"id": str(uuid.uuid4()), **target}
        item["name"] = self.name
        item["data_type"] = self.data_type
        if self.value is not None:
            item["value"] = self.value
        if self.data_type == "category":
            item["categories"] = [self.label]
        if self.range is not None:
            item["range_min"], item["range_max"] = self.range
        if self.version:
            item["version"] = self.version
        return json.dumps(item, ensure_ascii=False, separators=(",", ":"))


def refused(why: str, warn_once: Warn) -> None:
    warn_once(
        f"score:{why}", f"telmoni: a score was not sent: {why}", WarningInfo("input")
    )


def judged(
    name: object, value: object, given_range: object, version: object, warn_once: Warn
) -> Judgement | None:
    """The score, or None with one warning for one breaking its rules."""
    judgement = _judge(name, value, given_range, version)
    if isinstance(judgement, str):
        refused(judgement, warn_once)
        return None
    return judgement


def _judge(
    name: object, value: object, given_range: object, version: object
) -> Judgement | str:
    trimmed = _trimmed(name)
    if not 0 < len(trimmed) <= contract.SCORE_NAME_MAX:
        return NAME
    kept = ""
    if version is not None:
        kept = _trimmed(version)
        if not isinstance(version, str) or len(kept) > contract.SCORE_VERSION_MAX:
            return VERSION
    # A bool first: Python counts it an int.
    if isinstance(value, bool):
        if given_range is not None:
            return RANGE
        return Judgement(trimmed, "boolean", 1.0 if value else 0.0, version=kept)
    if isinstance(value, str):
        if given_range is not None:
            return RANGE
        label = _trimmed(value)
        if not 0 < len(label) <= contract.SCORE_CATEGORY_MAX:
            return CATEGORY
        return Judgement(trimmed, "category", label=label, version=kept)
    if not isinstance(value, (int, float)):
        return VALUE
    number = _float(value)
    if not math.isfinite(number):
        return NUMBER
    if given_range is None:
        return Judgement(trimmed, "number", number, version=kept)
    if not isinstance(given_range, (list, tuple)) or len(given_range) != 2:
        return NUMBER
    low, high = (_float(bound) for bound in given_range)
    finite = math.isfinite(low) and math.isfinite(high)
    if not (finite and low < high and low <= number <= high):
        return NUMBER
    return Judgement(trimmed, "number", number, range=(low, high), version=kept)


def target(
    run_id: object, span_id: object, session_id: object, warn_once: Warn
) -> dict[str, str] | None:
    """What a late score names, its ids as ingest reads them; None with one
    warning for anything else."""
    named = _target(run_id, span_id, session_id)
    if named is None:
        refused(TARGET, warn_once)
    return named


def _target(
    run_id: object, span_id: object, session_id: object
) -> dict[str, str] | None:
    if session_id is not None:
        if run_id is not None or span_id is not None or not isinstance(session_id, str):
            return None
        session = _trimmed(session_id)
        if not 0 < len(session) <= contract.SESSION_ID_MAX:
            return None
        return {"session_id": session}
    run = ids.run_uuid(run_id) if isinstance(run_id, str) else None
    # The nil run is no run's.
    if run is None or run.int == 0:
        return None
    named = {"run_id": str(run)}
    if span_id is not None:
        if not isinstance(span_id, str) or not _SPAN_ID.fullmatch(span_id):
            return None
        named["span_id"] = span_id.lower()
    return named


def _trimmed(text: object) -> str:
    """Unicode's White_Space off its ends; no string reads as none."""
    return (_unset_if_blank(text) or "") if isinstance(text, str) else ""


def _float(value: object) -> float:
    """A number as a float, an int past a float's reach an infinity; anything
    else NaN, which no check passes."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return math.nan
    try:
        return float(value)
    except OverflowError:
        return math.inf
