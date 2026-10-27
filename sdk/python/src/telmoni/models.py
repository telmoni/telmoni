from __future__ import annotations

import ipaddress
import math
import os
import string
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from datetime import datetime
from typing import TYPE_CHECKING, Any

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


_WHITE_SPACE = (
    "\t\n\v\f\r \x85\xa0\u1680"
    + "".join(map(chr, range(0x2000, 0x200B)))
    + "\u2028\u2029\u202f\u205f\u3000"
)
"""Unicode's White_Space, which an endpoint's ends are trimmed of, as Go and
Rust trim them; `str.isspace` takes U+001C to U+001F too."""
_NAME = frozenset(string.ascii_letters + string.digits + "._-")
_PATH = frozenset(string.ascii_letters + string.digits + "-._~!$&'()*+,;=:@/")
_IPV6 = frozenset(string.hexdigits + ":.")
_LOOPBACK = ipaddress.IPv4Network("127.0.0.0/8")


def _host_of(endpoint: str) -> str | None:
    """The host ``endpoint`` names, as written, or None where it is no URL every SDK
    reads alike (``sdk/fixtures/endpoints.json``), nothing a parser rewrites."""
    if not all("!" <= char <= "~" for char in endpoint):
        return None
    scheme, found, rest = endpoint.partition("://")
    authority, _, path = rest.partition("/")
    if (
        not found
        or scheme.lower() not in ("http", "https")
        or not set(path) <= _PATH
        or {".", ".."} & set(path.split("/"))
    ):
        return None
    if authority.startswith("["):
        end = authority.find("]") + 1
        host, port = authority[:end], authority[end:]
        if not end or not set(host[1:-1]) <= _IPV6 or _ipv6(host[1:-1]) is None:
            return None
    else:
        host, colon, digits = authority.partition(":")
        port = colon + digits
        # WHATWG's parser reads a name ending in a number as an IPv4 address.
        if (
            not host
            or not set(host) <= _NAME
            or not all(map(_label_ok, host.removesuffix(".").split(".")))
            or (_numbered(host) and _ipv4(host) is None)
        ):
            return None
    return host if _port_ok(port) else None


def _label_ok(label: str) -> bool:
    """A name's label as DNS and TLS take it: 1 to 63 characters, no hyphen at
    either end."""
    return 0 < len(label) <= 63 and label[0] != "-" and label[-1] != "-"


def _port_ok(port: str) -> bool:
    """No port, an empty one, or up to five digits from 1 to 65535."""
    if not port:
        return True
    digits = port[1:]
    within = len(digits) <= 5 and digits.isdigit() and 0 < int(digits) <= 65535
    return port[0] == ":" and (not digits or within)


def _numbered(host: str) -> bool:
    """Whether ``host``'s last label, a trailing dot aside, is a number."""
    last = host.removesuffix(".").rpartition(".")[2]
    hexadecimal = last[:2].lower() == "0x" and set(last[2:]) <= set(string.hexdigits)
    return last.isdigit() or hexadecimal


def _ipv4(text: str) -> ipaddress.IPv4Address | None:
    try:
        return ipaddress.IPv4Address(text)
    except ValueError:
        return None


def _ipv6(text: str) -> ipaddress.IPv6Address | None:
    try:
        return ipaddress.IPv6Address(text)
    except ValueError:
        return None


def _this_machine(host: str) -> bool:
    """Whether ``host``, as ``_host_of`` gives it, is ``localhost``, a name under it,
    127.0.0.0/8 or ::1, an IPv4-mapped address read as its IPv4."""
    if host.startswith("["):
        address = _ipv6(host[1:-1])
        if address is None:
            return False
        mapped = address.ipv4_mapped
        loopback = mapped is not None and mapped in _LOOPBACK
        return loopback or address == ipaddress.IPv6Address("::1")
    address4 = _ipv4(host)
    if address4 is not None:
        return address4 in _LOOPBACK
    name = host.lower()
    return name == "localhost" or name.endswith(".localhost")


def bypasses_proxy(url: str) -> bool:
    """Whether ``url`` goes straight to this machine, past any proxy the environment
    names: a proxy would take plain HTTP's key off it in the clear, and one on
    another machine cannot reach this one's ports anyway."""
    host = _host_of(url)
    return host is not None and _this_machine(host)


def endpoint_of(given: str | None) -> str:
    """The endpoint as the CLI reads one: a host without a scheme gets
    ``https://``, or ``http://`` for this machine."""
    text = (given or "").strip(_WHITE_SPACE)
    # Only what follows the "://" is trimmed, so a second reading changes nothing.
    head, scheme, tail = text.partition("://")
    if scheme:
        return head + scheme + _trim_tail(tail)
    endpoint = _trim_tail(text)
    if not endpoint:
        return DEFAULT_ENDPOINT
    # "https:/host" or "https//host" is a scheme mistyped, not a host: the check refuses it.
    if endpoint.split(":", 1)[0].split("/", 1)[0].lower() in ("http", "https"):
        return endpoint
    host = _host_of(f"http://{endpoint}")
    local = host is not None and _this_machine(host)
    return f"{'http' if local else 'https'}://{endpoint}"


def _trim_tail(text: str) -> str:
    """Trailing slashes and whitespace off, so the path appends cleanly."""
    end = len(text)
    while end and (text[end - 1] == "/" or text[end - 1] in _WHITE_SPACE):
        end -= 1
    return text[:end]


def check_endpoint(endpoint: str) -> None:
    """An endpoint the SDKs could read two ways fails an export or reaches another
    host, and plain HTTP to another machine would carry the key bare."""
    host = _host_of(endpoint)
    if host is None:
        raise ValueError("the endpoint must be an http:// or https:// URL")
    if endpoint[:7].lower() == "http://" and not _this_machine(host):
        raise ValueError(
            f"refusing to send the API key over unencrypted HTTP to '{host}'; use HTTPS"
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
    # The share of runs sent, decided per run in the processor and never through
    # the trace's sampled flag, so an application's own sampling is untouched.
    sample_rate: float = 1.0
    # The sampler whose root decision a run takes when it starts a trace of its
    # own; None reads it from the application's tracer provider, or samples.
    sampler: Sampler | None = field(default=None, repr=False)
    # The deployment this process runs in (production, staging) and the
    # release it runs (a version or a commit), sent on every span's resource
    # as deployment.environment.name and service.version; nothing when unset.
    environment: str | None = None
    release: str | None = None
    # The project's own prices, which a run's spend cap counts a call by
    # before the table this release carries; a name given twice takes its last.
    prices: Sequence[Price] = ()

    def __post_init__(self) -> None:
        self._normalize()

    def _normalize(self) -> None:
        """Blank values unset, the endpoint read as the CLI reads it, the key
        wrapped and every check, again as a client is built, for a field set since."""
        self.endpoint = endpoint_of(self.endpoint)
        check_endpoint(self.endpoint)
        self.organization_id = _unset_if_blank(self.organization_id)
        if isinstance(self.api_key, str):
            key = _unset_if_blank(self.api_key)
            self.api_key = Secret(key) if key is not None else None
        if not 0 < self.sample_rate <= 1:
            raise ValueError("sample_rate must be above 0 and up to 1")
        self.environment = _unset_if_blank(self.environment)
        self.release = _unset_if_blank(self.release)
        self.prices = tuple(self.prices)
        for price in self.prices:
            if not isinstance(price, Price):
                raise TypeError("a price must be a Price")
            price.check()

    def bearer(self) -> str | None:
        """The key, for the one header that carries it."""
        return self.api_key.expose() if isinstance(self.api_key, Secret) else None

    @classmethod
    def from_env(cls) -> Config:
        """Read ``TELMONI_ENDPOINT``, ``TELMONI_API_KEY``, ``TELMONI_ORG``, and
        ``TELMONI_ENVIRONMENT`` and ``TELMONI_RELEASE`` for the deployment.

        A blank value is unset, as the CLI reads them. No ``OTEL_*`` variable is
        read: the SDK's tracer provider is built part by part.
        """
        return cls(
            endpoint=os.getenv("TELMONI_ENDPOINT") or DEFAULT_ENDPOINT,
            api_key=os.getenv("TELMONI_API_KEY"),
            organization_id=os.getenv("TELMONI_ORG"),
            environment=os.getenv("TELMONI_ENVIRONMENT"),
            release=os.getenv("TELMONI_RELEASE"),
        )


@dataclass(frozen=True)
class Usage:
    """A model call's tokens, as the conventions count them: input includes the
    cache's reads and writes, output reasoning; sum parts reported apart. Each
    goes as a whole number, a fraction cut, one below 0 or past 64 bits left
    off the call with one warning. ``cost`` is what the call cost in US
    dollars, where the provider reports it; one below 0 or not finite is left
    off with one warning, and a call without one is priced by its tokens."""

    input_tokens: int | None = None
    output_tokens: int | None = None
    cache_read_input_tokens: int | None = None
    cache_write_input_tokens: int | None = None
    cost: float | None = None


@dataclass(frozen=True)
class Price:
    """A project's own price for one of a provider's models, as the provider
    and the model are sent: each in US dollars a token, a cache price not
    given being the input price."""

    provider: str
    model: str
    input: float
    output: float
    cache_read: float | None = None
    cache_write: float | None = None

    def check(self) -> None:
        """A name a call can carry, and prices that are numbers from 0."""
        for name in (self.provider, self.model):
            if not isinstance(name, str) or not name:
                raise ValueError("a price names its provider and its model")
        given = [self.input, self.output]
        given += [p for p in (self.cache_read, self.cache_write) if p is not None]
        if any(_dollars(price) is None for price in given):
            raise ValueError("a price is a finite number of US dollars a token, from 0")


def _dollars(value: object) -> float | None:
    """An amount of US dollars as a float, whatever number it was given as; None
    for one below 0, not finite, or no number, a `bool` or text among them."""
    if isinstance(value, (bool, str, bytes)):
        return None
    try:
        dollars = float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError, OverflowError):
        return None
    return dollars if math.isfinite(dollars) and dollars >= 0 else None


@dataclass(frozen=True)
class ModelCallRecord:
    """A model call that has ended: its model, its times and its tokens."""

    model: str
    start: datetime
    end: datetime
    provider: str | None = None
    operation: str = "chat"
    usage: Usage | None = None
    error_type: str | None = None
    cancelled: bool = False


@dataclass(frozen=True)
class ToolCallRecord:
    """A tool call that has ended: its name and its times."""

    name: str
    start: datetime
    end: datetime
    error_type: str | None = None
    cancelled: bool = False


CallRecord = ModelCallRecord | ToolCallRecord


@dataclass(frozen=True)
class StepRecord:
    """A step that has ended, with the calls made inside it."""

    name: str
    start: datetime
    end: datetime
    calls: Sequence[CallRecord] = ()
    error_type: str | None = None
    cancelled: bool = False


@dataclass(frozen=True)
class RunRecord:
    """A run that has ended, as ``record_run`` sends it: its id, a UUID the
    caller gives, and everything in it, each with its own times. Times carry
    their timezone, since a naive one would be read as this machine's."""

    run_id: str
    agent: str
    start: datetime
    end: datetime
    user_id: str | None = None
    session_id: str | None = None
    tags: Mapping[str, str] | None = None
    inside: Sequence[StepRecord | CallRecord] = ()
    error_type: str | None = None
    cancelled: bool = False
