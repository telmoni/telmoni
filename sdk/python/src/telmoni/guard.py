"""The guard: a run's opt-in caps on its steps and its spend, counted in this
process with no call to Telmoni, raised at the step, model call or tool call
the run would open past one.

A call's cost is counted as the server counts it: as reported, else its tokens
at its price in the table this release carries (``model_prices.json``,
LiteLLM's, whose notice it holds), else nothing; so a call with neither stops
nothing. A price handed to the client comes before the table's, and the server
never sees it.
"""

from __future__ import annotations

import json
import threading
from collections.abc import Iterable, Mapping
from decimal import Decimal
from importlib import resources
from typing import NamedTuple

from .models import Price, _dollars


class CapExceededError(RuntimeError):
    """A run reached a cap it was opened with, raised at what it would open next:
    ``cap`` is ``steps`` or ``spend``, ``limit`` the cap and ``used`` what the
    run had used of it, its steps opened or the US dollars it spent."""

    def __init__(self, cap: str, limit: float, used: float) -> None:
        # Its three arguments, so a pickle builds it again.
        super().__init__(cap, limit, used)
        self.cap = cap
        self.limit = limit
        self.used = used

    def __str__(self) -> str:
        if self.cap == "steps":
            return f"the run reached its step cap of {_amount(self.limit)}"
        return (
            f"the run reached its spend cap of ${_amount(self.limit)}, "
            f"having spent ${_amount(self.used)}"
        )


def _amount(value: float) -> str:
    """The shortest decimal that reads back as `value`, with no exponent, as
    the other SDKs print it."""
    return f"{Decimal(repr(float(value))).normalize():f}"


class Rate(NamedTuple):
    """One name's price, each in US dollars a token."""

    input: float
    output: float
    cache_read: float
    cache_write: float

    def cost(self, tokens: Tokens) -> float:
        """What a call's tokens cost: its input less the cache's at the input
        price, each cache count at its own, and its output at the output price."""
        uncached = max(tokens.input - tokens.cache_read - tokens.cache_write, 0)
        return (
            uncached * self.input
            + tokens.cache_read * self.cache_read
            + tokens.cache_write * self.cache_write
            + tokens.output * self.output
        )


class Tokens(NamedTuple):
    """A call's counts as sent, one left off counting 0."""

    input: int = 0
    output: int = 0
    cache_read: int = 0
    cache_write: int = 0

    def some(self) -> bool:
        """Whether there is a token to price."""
        return any(self)


_table: dict[tuple[str, str], Rate] | None = None
_table_lock = threading.Lock()


def table() -> Mapping[tuple[str, str], Rate]:
    """The price table this release carries, read once per process. A test
    holds the file to the core's, so it is always read; were it not, nothing
    would be priced."""
    global _table
    with _table_lock:
        if _table is None:
            try:
                text = resources.files(__package__).joinpath("model_prices.json")
                _table = {
                    (row["provider"], row["model"]): Rate(
                        row["input"],
                        row["output"],
                        row["cache_read"],
                        row["cache_write"],
                    )
                    for row in json.loads(text.read_text(encoding="utf-8"))["prices"]
                }
            except (OSError, ValueError, KeyError, TypeError):
                _table = {}
        return _table


def own(prices: Iterable[Price]) -> dict[tuple[str, str], Rate]:
    """A project's own prices by name, a name given twice its last."""
    return {
        (price.provider, price.model): Rate(
            float(price.input),
            float(price.output),
            float(price.input if price.cache_read is None else price.cache_read),
            float(price.input if price.cache_write is None else price.cache_write),
        )
        for price in prices
    }


def priced(
    own_prices: Mapping[tuple[str, str], Rate],
    provider: str,
    model: str,
    tokens: Tokens,
) -> float | None:
    """What a call that reported no cost cost: at its own price, else at the
    table's, by its provider and model as sent, whole; None for a name
    neither holds."""
    rate = own_prices.get((provider, model))
    if rate is None:
        rate = table().get((provider, model))
    return None if rate is None else rate.cost(tokens)


class Guard:
    """One run's caps, and what it has used of them."""

    def __init__(self, max_steps: int | None, max_spend: float | None) -> None:
        self._lock = threading.Lock()
        self._max_steps = max_steps
        self._max_spend = max_spend
        self._steps = 0
        self._spent = 0.0

    @property
    def spent(self) -> float:
        with self._lock:
            return self._spent

    @property
    def capped(self) -> bool:
        """Whether the run has a spend cap, which a call priced at nothing
        never reaches."""
        return self._max_spend is not None

    def opens(self, *, step: bool) -> None:
        """Count a step opening, or raise where the run reached a cap: its
        spend for whatever opens, its steps for a step."""
        with self._lock:
            if self._max_spend is not None and self._spent >= self._max_spend:
                raise CapExceededError("spend", self._max_spend, self._spent)
            if step:
                if self._max_steps is not None and self._steps >= self._max_steps:
                    raise CapExceededError("steps", self._max_steps, self._steps)
                self._steps += 1

    def counts(self, before: float, now: float) -> None:
        """A call's cost counted again: ``now`` in place of ``before``."""
        with self._lock:
            self._spent += now - before


def caps(max_steps: object, max_spend: object) -> tuple[int | None, float | None]:
    """A run's caps, or why they are none: whole steps above 0, and a spend of
    US dollars above 0, read as a cost is."""
    if max_steps is not None and (
        isinstance(max_steps, bool) or not isinstance(max_steps, int) or max_steps < 1
    ):
        raise ValueError("max_steps must be a whole number above 0")
    spend = None if max_spend is None else _dollars(max_spend)
    if max_spend is not None and not spend:
        raise ValueError("max_spend must be a finite number of US dollars above 0")
    return max_steps, spend
