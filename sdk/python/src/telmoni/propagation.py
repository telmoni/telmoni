"""A run across services: the W3C ``traceparent`` of the span current inside
a run, and the run's id and agent as the W3C ``baggage`` members
``telmoni.run_id`` and ``telmoni.agent``, written into an outgoing request's
headers and read back from an incoming one's. The SDK's own helpers, never a
propagator set globally: an application's propagators are its own.
"""

from __future__ import annotations

import re
from collections.abc import Mapping, MutableMapping
from dataclasses import dataclass

from opentelemetry.trace import SpanContext, TraceFlags

from . import contract

_UUID = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", re.IGNORECASE
)
_HEX = re.compile(r"^[0-9a-f]+$")


@dataclass(frozen=True)
class Continued:
    """What headers name: the span to continue under, and the run."""

    span_context: SpanContext
    run_id: str
    agent: str


def write_headers(
    headers: MutableMapping[str, str], span: SpanContext, run_id: str, agent: str
) -> None:
    """``span`` and the run as the next service reads them, beside the
    members ``headers`` already hold."""
    sampled = "01" if span.trace_flags & TraceFlags.SAMPLED else "00"
    _set_header(
        headers,
        contract.TRACEPARENT,
        f"00-{span.trace_id:032x}-{span.span_id:016x}-{sampled}",
    )
    members = [
        member.strip()
        for member in (_get_header(headers, contract.BAGGAGE) or "").split(",")
    ]
    kept = [m for m in members if m and not _is_ours(m)]
    kept.append(f"{contract.RUN_ID}={run_id}")
    kept.append(f"{contract.AGENT}={encode_baggage_value(agent)}")
    _set_header(headers, contract.BAGGAGE, ",".join(kept))


def read_headers(headers: Mapping[str, str]) -> Continued | None:
    """The run ``headers`` name, or ``None`` for headers that name none."""
    traceparent = _get_header(headers, contract.TRACEPARENT)
    if traceparent is None:
        return None
    span_context = parse_traceparent(traceparent)
    if span_context is None:
        return None
    members = _baggage_members(_get_header(headers, contract.BAGGAGE) or "")
    run_id = members.get(contract.RUN_ID)
    agent = members.get(contract.AGENT)
    if run_id is None or not _UUID.match(run_id) or not agent:
        return None
    return Continued(span_context, run_id.lower(), agent)


def parse_traceparent(header: str) -> SpanContext | None:
    """A ``traceparent`` as the W3C spells it: a version that is not ``ff``,
    a trace id and a span id that are not zeros, and the flags; a version
    other than ``00`` may be followed by more, which is ignored."""
    parts = header.strip().split("-")
    if len(parts) < 4:
        return None
    version, trace_id, span_id, flags = parts[:4]
    if not _is_hex(version, 2) or version == "ff":
        return None
    if version == "00" and len(parts) != 4:
        return None
    if not _is_hex(trace_id, 32) or int(trace_id, 16) == 0:
        return None
    if not _is_hex(span_id, 16) or int(span_id, 16) == 0:
        return None
    if not _is_hex(flags, 2):
        return None
    return SpanContext(
        trace_id=int(trace_id, 16),
        span_id=int(span_id, 16),
        is_remote=True,
        trace_flags=TraceFlags(int(flags, 16) & TraceFlags.SAMPLED),
    )


def _baggage_members(header: str) -> dict[str, str]:
    """The baggage's members by key, each value percent-decoded, its
    properties left off."""
    members: dict[str, str] = {}
    for member in header.split(","):
        key, equals, rest = member.partition("=")
        if not equals:
            continue
        key = key.strip()
        value = rest.split(";", 1)[0].strip()
        if key:
            members[key] = decode_baggage_value(value)
    return members


def _is_ours(member: str) -> bool:
    key = member.split("=", 1)[0].strip()
    return key in (contract.RUN_ID, contract.AGENT)


def _is_hex(text: str, length: int) -> bool:
    return len(text) == length and _HEX.match(text) is not None


def encode_baggage_value(value: str) -> str:
    """A baggage value as the W3C writes it: the printable ASCII its grammar
    allows as it is, and every other byte of the UTF-8 percent-encoded, so a
    space, a comma or a name in another script travels whole."""
    out = []
    for byte in value.encode("utf-8"):
        out.append(chr(byte) if _allowed_in_baggage(byte) else f"%{byte:02X}")
    return "".join(out)


def decode_baggage_value(value: str) -> str:
    """A baggage value read back: each ``%XX`` its byte, a ``%`` without two
    hex digits kept as it is."""
    out = bytearray()
    i = 0
    while i < len(value):
        pair = value[i + 1 : i + 3]
        if value[i] == "%" and len(pair) == 2 and _HEX.match(pair.lower()):
            out.append(int(pair, 16))
            i += 3
        else:
            out.extend(value[i].encode("utf-8"))
            i += 1
    return out.decode("utf-8", errors="replace")


def _allowed_in_baggage(byte: int) -> bool:
    """The W3C's ``baggage-octet``: printable ASCII but space, ``"``, ``,``,
    ``;`` and ``\\``; and but ``%``, which the grammar allows and a reader
    would decode."""
    return (
        byte == 0x21
        or (0x23 <= byte <= 0x2B and byte != 0x25)
        or 0x2D <= byte <= 0x3A
        or 0x3C <= byte <= 0x5B
        or 0x5D <= byte <= 0x7E
    )


def _get_header(headers: Mapping[str, str], name: str) -> str | None:
    """The header named ``name``, however its key is cased."""
    for key, value in headers.items():
        if key.lower() == name:
            return value
    return None


def _set_header(headers: MutableMapping[str, str], name: str, value: str) -> None:
    """The header named ``name`` set once, whatever case its key was in."""
    for key in [key for key in headers if key.lower() == name]:
        del headers[key]
    headers[name] = value
