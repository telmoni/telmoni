from __future__ import annotations

import ipaddress
import os
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any
from urllib.parse import urlsplit

if TYPE_CHECKING:
    from opentelemetry.sdk.trace.sampling import Sampler

DEFAULT_ENDPOINT = "https://api.telmoni.com"
"""The machine host; the console's, ``https://telmoni.com``, serves people."""


class Secret:
    """A value that prints as ``<redacted>`` wherever it is shown.

    ``dataclasses.asdict`` deep-copies what it does not recurse into, so a
    configuration turned into a dict carries this, never the key; only
    ``expose()`` gives the key back.
    """

    __slots__ = ("_value",)

    def __init__(self, value: str) -> None:
        self._value = value

    def expose(self) -> str:
        return self._value

    def __repr__(self) -> str:
        return "Secret(<redacted>)"

    def __str__(self) -> str:
        return "<redacted>"

    def __eq__(self, other: object) -> bool:
        return isinstance(other, Secret) and other._value == self._value

    def __hash__(self) -> int:
        return hash(self._value)

    # Immutable, so a copy is itself: ``dataclasses.asdict`` deep-copies it
    # and still holds a Secret. Pickling is the one serialization that would
    # carry the key out, so it is refused.
    def __copy__(self) -> Secret:
        return self

    def __deepcopy__(self, memo: dict[int, Any]) -> Secret:
        return self

    def __reduce_ex__(self, protocol: Any) -> Any:
        raise TypeError("a Secret is not pickled")


def _unset_if_blank(value: str | None) -> str | None:
    if value is None or not value.strip():
        return None
    return value.strip()


def is_loopback_host(host: str) -> bool:
    """Whether ``host`` is this machine: ``localhost``, a name under it, or a
    loopback address; an IPv6 address comes in brackets."""
    bare = host.strip("[]")
    try:
        return ipaddress.ip_address(bare).is_loopback
    except ValueError:
        return host == "localhost" or host.endswith(".localhost")


def refuse_cleartext(endpoint: str) -> None:
    """Plain HTTP reaches only this machine: every export carries the key."""
    parts = urlsplit(endpoint)
    if parts.scheme == "http" and not is_loopback_host(parts.hostname or ""):
        raise ValueError(
            "refusing to send the API key over unencrypted HTTP to "
            f"'{parts.hostname or ''}'; use HTTPS"
        )


@dataclass
class Config:
    """How the SDK reaches Telmoni, and what it sends."""

    endpoint: str = DEFAULT_ENDPOINT
    organization_id: str | None = None
    # Wrapped in a Secret by __post_init__, and out of the repr: a configuration
    # printed, logged or turned into a dict would otherwise carry the key.
    api_key: Secret | str | None = field(default=None, repr=False)
    # Content (prompts, completions, tool arguments and results) leaves the
    # process only when the code asks here and the project's mode allows.
    capture_content: bool = False
    # The share of runs sent, decided per run in the exporter and never through
    # the trace's sampled flag, so an application's own sampling is untouched.
    sample_rate: float = 1.0
    # The sampler whose root decision a run takes when it starts a trace of its
    # own; None reads it from the application's tracer provider, or samples.
    sampler: Sampler | None = field(default=None, repr=False)

    def __post_init__(self) -> None:
        endpoint = _unset_if_blank(self.endpoint)
        self.endpoint = (endpoint or DEFAULT_ENDPOINT).rstrip("/")
        refuse_cleartext(self.endpoint)
        self.organization_id = _unset_if_blank(self.organization_id)
        if isinstance(self.api_key, str):
            key = _unset_if_blank(self.api_key)
            self.api_key = Secret(key) if key is not None else None
        if not 0 < self.sample_rate <= 1:
            raise ValueError("sample_rate must be above 0 and up to 1")

    def bearer(self) -> str | None:
        """The key, for the one header that carries it."""
        return self.api_key.expose() if isinstance(self.api_key, Secret) else None

    @classmethod
    def from_env(cls) -> Config:
        """Read ``TELMONI_ENDPOINT``, ``TELMONI_API_KEY`` and ``TELMONI_ORG``.

        A blank value is unset, as the CLI reads them. No ``OTEL_*`` variable is
        read: the SDK's tracer provider is built part by part.
        """
        return cls(
            endpoint=os.getenv("TELMONI_ENDPOINT") or DEFAULT_ENDPOINT,
            api_key=os.getenv("TELMONI_API_KEY"),
            organization_id=os.getenv("TELMONI_ORG"),
        )


@dataclass(frozen=True)
class Bounds:
    """What a monitor holds a run to; each optional."""

    stall_after_s: int | None = None
    max_steps: int | None = None
    max_cost_usd: float | None = None


@dataclass(frozen=True)
class Usage:
    """A model call's tokens, counted as the conventions count them.

    Input tokens include the cache's reads and writes; output tokens include
    reasoning. Where a provider reports parts apart, sum them first.
    """

    input_tokens: int | None = None
    output_tokens: int | None = None
    cache_read_input_tokens: int | None = None
    cache_write_input_tokens: int | None = None
