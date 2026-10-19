"""The exporter: OTLP's JSON encoding over the standard library.

It takes content off every span unless the code asked and the project's mode
allows, sends to ``POST {endpoint}/otlp/v1/traces``, waits out a
``Retry-After``, and reads ``Telmoni-Content-Mode`` and the partial success
from each answer, warning once for each attribute an answer names dropped.
"""

from __future__ import annotations

import contextlib
import functools
import http.client
import json
import logging
import math
import re
import socket
import ssl
import threading
import time
import urllib.error
import urllib.request
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from importlib import metadata
from typing import Any

from opentelemetry.sdk.trace import ReadableSpan
from opentelemetry.trace import SpanKind, StatusCode

from . import contract
from ._version import __version__
from .models import Config

logger = logging.getLogger("telmoni")

ATTEMPTS = 3
"""How many times one batch is sent before it is dropped."""
REQUEST_TIMEOUT_S = 10.0
ANSWER_MAX = 1 << 20
"""How much of an answer is read: a problem's JSON is small, and an export
never pays for more."""
RETRY_AFTER_MAX_S = 60.0
WARNED_MAX = 1024
"""How many distinct warnings are remembered before the set starts over."""
CONTENT_DOCS = "https://telmoni.com/docs/api/ingest#what-is-dropped-at-the-door"
"""Where a dropped content field's warning sends the reader."""

_SURROGATE = re.compile("[\ud800-\udfff]")
_DROPPED = re.compile(r"^span (\S+) dropped (\S+): (\w+)(?:; (.*))?$")
_REFUSED = re.compile(r"^span (\S+) refused: (.*)$")

# OTLP numbers the kinds from 1; OpenTelemetry's Python enum from 0.
_KINDS = {
    SpanKind.INTERNAL: 1,
    SpanKind.SERVER: 2,
    SpanKind.CLIENT: 3,
    SpanKind.PRODUCER: 4,
    SpanKind.CONSUMER: 5,
}


@dataclass(frozen=True)
class Answer:
    status: int
    headers: Mapping[str, str]
    body: bytes

    def header(self, name: str) -> str | None:
        wanted = name.lower()
        for key, value in self.headers.items():
            if key.lower() == wanted:
                return value
        return None


Transport = Callable[[str, Mapping[str, str], bytes], Answer]
"""Sends one request; raises ``OSError`` when the platform could not be reached."""


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is answered as the 3xx it is: followed, it would resend the
    bearer to wherever it points."""

    def redirect_request(self, *args: Any, **kwargs: Any) -> None:
        return None


class _Deadline:
    """A request's ten seconds from its start, held once it is connected: a
    socket's timeout is each read's alone, which a trickled answer outlasts."""

    def __init__(self, seconds: float) -> None:
        self._lock = threading.Lock()
        self._sockets: list[socket.socket] = []
        self._over = False
        self.expired = False
        self._timer = threading.Timer(seconds, self._expire)
        self._timer.daemon = True
        self._timer.start()

    def watch(self, sock: socket.socket) -> None:
        # A descriptor of its own, which no socket opened since can be given,
        # as the connection's may be once the answer's read closes it.
        held = socket.fromfd(sock.fileno(), sock.family, sock.type, sock.proto)
        with self._lock:
            if self._over:
                held.close()
                return
            self._sockets.append(held)
            if self.expired:
                _shut(held)

    def _expire(self) -> None:
        with self._lock:
            if self._over:
                return
            self.expired = True
            for sock in self._sockets:
                _shut(sock)

    def end(self) -> None:
        """Stop watching, and let go of the descriptors held."""
        with self._lock:
            self._over = True
            for sock in self._sockets:
                sock.close()
            self._sockets.clear()
        self._timer.cancel()


def _shut(sock: socket.socket) -> None:
    # Shuts the connection for every descriptor of it, a TLS socket's too.
    with contextlib.suppress(OSError):
        sock.shutdown(socket.SHUT_RDWR)


def _watched(
    base: type[http.client.HTTPConnection], deadline: _Deadline
) -> type[http.client.HTTPConnection]:
    class Watched(base):  # type: ignore[valid-type,misc]
        def connect(self) -> None:
            super().connect()
            deadline.watch(self.sock)

    return Watched


class _Plain(urllib.request.HTTPHandler):
    def __init__(self, deadline: _Deadline) -> None:
        super().__init__()
        self._deadline = deadline

    def do_open(self, http_class: Any, req: Any, **kwargs: Any) -> Any:
        return super().do_open(_watched(http_class, self._deadline), req, **kwargs)


class _Tls(urllib.request.HTTPSHandler):
    def __init__(self, deadline: _Deadline) -> None:
        super().__init__(context=_tls())
        self._deadline = deadline

    def do_open(self, http_class: Any, req: Any, **kwargs: Any) -> Any:
        return super().do_open(_watched(http_class, self._deadline), req, **kwargs)


@functools.cache
def _tls() -> ssl.SSLContext:
    """One context for every export, made as urllib makes its own, through the
    hook an application may set its trust by; each loads the certificates."""
    context = ssl._create_default_https_context()
    context.set_alpn_protocols(["http/1.1"])
    return context


def _urllib_transport(url: str, headers: Mapping[str, str], body: bytes) -> Answer:
    request = urllib.request.Request(
        url, data=body, headers=dict(headers), method="POST"
    )
    deadline = _Deadline(REQUEST_TIMEOUT_S)
    opener = urllib.request.build_opener(
        _NoRedirect(), _Plain(deadline), _Tls(deadline)
    )
    try:
        try:
            response = opener.open(request, timeout=REQUEST_TIMEOUT_S)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            try:
                return Answer(
                    response.status, dict(response.headers.items()), _body(response)
                )
            finally:
                deadline.end()
    except (OSError, http.client.HTTPException) as error:
        if deadline.expired:
            raise TimeoutError("the answer outlasted the request's deadline") from error
        raise
    finally:
        deadline.end()


def _body(response: Any) -> bytes:
    """At most ``ANSWER_MAX`` bytes of the answer's body."""
    body: bytes = response.read(ANSWER_MAX)
    if len(body) < ANSWER_MAX:
        # Read to the end, which raises `IncompleteRead` for a body cut short
        # as `read()` would: a read of a given size returns what came.
        response.read()
    return body


def _sdk_version() -> str:
    try:
        return metadata.version("opentelemetry-sdk")
    except metadata.PackageNotFoundError:
        return "unknown"


class Exporter:
    def __init__(
        self,
        config: Config,
        *,
        transport: Transport | None = None,
        sleep: Callable[[float], None] | None = None,
    ) -> None:
        self._config = config
        self._transport = transport or _urllib_transport
        self._sleep = sleep or time.sleep
        self._url = f"{config.endpoint}/otlp/v1/traces"
        self._content_mode: str | None = None
        self._warned: set[str] = set()
        self._resource = [
            _attribute(contract.DISTRO_NAME, contract.DISTRO),
            _attribute("telemetry.distro.version", __version__),
            _attribute("telemetry.sdk.language", "python"),
            _attribute("telemetry.sdk.name", "opentelemetry"),
            _attribute("telemetry.sdk.version", _sdk_version()),
        ]
        # The deployment, where the configuration names one: never from
        # OTEL_RESOURCE_ATTRIBUTES, which the application's exporters read.
        if config.environment is not None:
            self._resource.append(_attribute(contract.ENVIRONMENT, config.environment))
        if config.release is not None:
            self._resource.append(_attribute(contract.RELEASE, config.release))

    @property
    def content_mode(self) -> str | None:
        """The project's mode, as the last answer named it; None before one."""
        return self._content_mode

    def export(self, spans: Sequence[ReadableSpan]) -> None:
        if not spans:
            return
        # Python writes NaN and Infinity, which are no JSON: refused here, never sent.
        text = json.dumps(
            self.encode(spans),
            separators=(",", ":"),
            allow_nan=False,
            ensure_ascii=False,
        )
        # A lone surrogate is no text ingest reads, and would lose the batch.
        body = _SURROGATE.sub("\ufffd", text).encode()
        headers = {
            "Content-Type": "application/json",
            "User-Agent": f"telmoni-python/{__version__}",
        }
        bearer = self._config.bearer()
        if bearer is not None:
            headers["Authorization"] = f"Bearer {bearer}"
        failure = ""
        for attempt in range(ATTEMPTS):
            try:
                answer = self._transport(self._url, headers, body)
            except (OSError, http.client.HTTPException) as error:
                # A connection that failed, or one that broke mid-answer
                # (`IncompleteRead`, `BadStatusLine`): both a transport fault
                # the next attempt may not meet.
                failure = type(error).__name__
                self._wait(None, attempt)
                continue
            if 200 <= answer.status < 300:
                self._read(answer)
                return
            if answer.status in (429, 503):
                failure = f"{answer.status} {_problem_type(answer.body)}"
                self._wait(answer.header("Retry-After"), attempt)
                continue
            self.warn_once(
                f"status:{answer.status}",
                f"telmoni: export refused with {answer.status} "
                f"{_problem_type(answer.body)}; the batch was dropped",
            )
            return
        self.warn_once(
            f"failed:{failure}",
            f"telmoni: export failed after {ATTEMPTS} attempts ({failure}); "
            "the batch was dropped",
        )

    def encode(self, spans: Sequence[ReadableSpan]) -> dict[str, Any]:
        """OTLP's ``ExportTraceServiceRequest``, in its JSON encoding."""
        return {
            "resourceSpans": [
                {
                    "resource": {"attributes": self._resource},
                    "scopeSpans": [
                        {
                            "scope": {"name": contract.SCOPE, "version": __version__},
                            "spans": [self._span(span) for span in spans],
                        }
                    ],
                }
            ]
        }

    def _span(self, span: ReadableSpan) -> dict[str, Any]:
        keep_content = self._config.capture_content and self._content_mode == "on"
        attributes = [
            _attribute(key, value)
            for key, value in (span.attributes or {}).items()
            if keep_content or key not in contract.CONTENT
        ]
        encoded: dict[str, Any] = {
            "traceId": format(span.context.trace_id, "032x"),
            "spanId": format(span.context.span_id, "016x"),
            "name": span.name,
            "kind": _KINDS.get(span.kind, 1),
            "startTimeUnixNano": _nanos(span.start_time),
            "endTimeUnixNano": _nanos(span.end_time or span.start_time),
            "attributes": attributes,
        }
        if span.parent is not None and span.parent.is_valid:
            encoded["parentSpanId"] = format(span.parent.span_id, "016x")
        # Events and the status message are where an exception writes its
        # message and stack trace: never sent.
        if span.status.status_code is StatusCode.ERROR:
            encoded["status"] = {"code": 2}
        return encoded

    def _read(self, answer: Answer) -> None:
        mode = answer.header(contract.CONTENT_MODE_HEADER)
        if mode:
            self._content_mode = mode
        try:
            decoded = json.loads(answer.body or b"{}")
        except ValueError:
            return
        partial = decoded.get("partialSuccess") if isinstance(decoded, dict) else None
        if not isinstance(partial, dict):
            return
        message = partial.get("errorMessage")
        if isinstance(message, str) and message:
            self._note(message)

    def _note(self, message: str) -> None:
        for line in message.splitlines():
            dropped = _DROPPED.match(line)
            if dropped:
                _span_id, attribute, reason, rest = dropped.groups()
                text = f"telmoni: the server dropped {attribute} ({reason}"
                if rest:
                    text += f"; {rest}"
                text += f"); {_remedy(attribute, reason)}"
                self.warn_once(f"dropped:{attribute}:{reason}", text)
                continue
            refused = _REFUSED.match(line)
            if refused:
                why = refused.group(2)
                self.warn_once(
                    f"refused:{why}", f"telmoni: the server refused a span: {why}"
                )
                continue
            if line.startswith("and ") or not line:
                continue
            self.warn_once(f"line:{line}", f"telmoni: the server said: {line}")

    def _wait(self, retry_after: str | None, attempt: int) -> None:
        """Before the next attempt; nothing after the last."""
        if attempt + 1 >= ATTEMPTS:
            return
        seconds = float(2**attempt)
        if retry_after is not None:
            try:
                seconds = float(int(retry_after))
            except ValueError:
                pass
        self._sleep(min(max(seconds, 0.0), RETRY_AFTER_MAX_S))

    def warn_once(self, key: str, text: str) -> None:
        """Log ``text`` once per client under ``key``, as every warning is."""
        if key in self._warned:
            return
        # Bounded: a server naming ever-new attributes would otherwise grow it
        # for the life of the process; past the cap, warnings may repeat.
        if len(self._warned) >= WARNED_MAX:
            self._warned.clear()
        self._warned.add(key)
        warn(text)


def warn(text: str, *args: object) -> None:
    """Log a warning; a handler or filter that raises must not raise into the
    agent's own call, nor end the worker."""
    with contextlib.suppress(Exception):
        logger.warning(text, *args)


def _remedy(attribute: str, reason: str) -> str:
    if reason == "content":
        # The page, not a setting to change: which modes keep content is the
        # platform's to say, and no project can choose one that does yet.
        return f"the project's content mode decides whether content is kept: {CONTENT_DOCS}"
    if reason == "unlisted":
        return f"send it as the tag {contract.TAG_PREFIX}{attribute} to keep it"
    if reason == "over_cap":
        return "shorten it to keep it"
    return "see the server's answer"


def _problem_type(body: bytes) -> str:
    try:
        decoded = json.loads(body or b"{}")
    except ValueError:
        return ""
    message = decoded.get("message") if isinstance(decoded, dict) else None
    if not isinstance(message, str):
        return ""
    return message.split(":", 1)[0] if message.startswith("/errors/") else ""


def _nanos(at: int | None) -> str:
    """A span's time as OTLP writes it; 0 for one outside 64 bits, a clock
    set wrong, so ingest refuses that span rather than the whole export."""
    return str(at) if at is not None and 0 <= at < 2**64 else "0"


def _attribute(key: str, value: object) -> dict[str, Any]:
    """One OTLP ``KeyValue``: 64-bit integers travel as decimal strings."""
    encoded: dict[str, Any]
    if isinstance(value, bool):
        encoded = {"boolValue": value}
    elif isinstance(value, int):
        # An `(int, Enum)`'s own `str()` is its name, which ingest's parser refuses.
        encoded = {"intValue": str(int(value))}
    elif isinstance(value, float):
        # NaN or an infinity as null, as JavaScript and serde write one, which
        # ingest reads as no value: the attribute lost, never the batch.
        encoded = {"doubleValue": value if math.isfinite(value) else None}
    elif isinstance(value, str):
        encoded = {"stringValue": value}
    elif isinstance(value, (list, tuple)):
        encoded = {
            "arrayValue": {"values": [_attribute("", v)["value"] for v in value]}
        }
    else:
        encoded = {"stringValue": str(value)}
    return {"key": key, "value": encoded}
