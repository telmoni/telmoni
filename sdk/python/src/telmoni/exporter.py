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
from typing import Any, cast

from opentelemetry.sdk.trace import ReadableSpan
from opentelemetry.trace import SpanKind, StatusCode

from . import contract
from ._version import __version__
from .models import (
    Config,
    WarningInfo,
    _unset_if_blank,
    bypasses_proxy,
    header,
    replace_surrogates,
)

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

_DROPPED = re.compile(r"^span (\S+) dropped (\S+): (\w+)(?:; (.*))?$")
_REFUSED = re.compile(r"^span (\S+) refused: (.*)$")
_SECONDS = re.compile(r"[ \t]*([+-]?)([0-9]+)[ \t]*")

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
        return header(self.headers, name)


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


class _Lines:
    """A connection's reader, the last line it gave kept."""

    def __init__(self, fp: Any) -> None:
        self.fp = fp
        self.last: bytes | None = None

    def readline(self, limit: int = -1) -> bytes:
        self.last = self.fp.readline(limit)
        return self.last

    def __getattr__(self, name: str) -> Any:
        return getattr(self.fp, name)


class _WholeHead(http.client.HTTPResponse):
    """An answer past every 1xx before it but a 101, as Go and Rust read one, its head
    whole: one the connection's close or the deadline cuts short is no answer."""

    def begin(self) -> None:
        lines = _Lines(self.fp)
        # The head is read through it alone; the body, after, from the reader itself.
        self.fp = cast(Any, lines)
        try:
            super().begin()
            # `http.client` passes a 100 alone, and reads again only once it holds no headers.
            while 101 < self.status < 200 and lines.last != b"":
                self.headers = self.msg = cast(Any, None)
                super().begin()
        finally:
            # Unless `http.client` closed it, as it does before a bad status line.
            if cast(Any, self.fp) is lines:
                self.fp = lines.fp
        if lines.last == b"":
            raise http.client.RemoteDisconnected("the answer's head was cut short")


def _watched(
    base: type[http.client.HTTPConnection], deadline: _Deadline
) -> type[http.client.HTTPConnection]:
    class Watched(base):  # type: ignore[valid-type,misc]
        response_class = _WholeHead

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
    handlers: list[urllib.request.BaseHandler] = [
        _NoRedirect(),
        _Plain(deadline),
        _Tls(deadline),
    ]
    if bypasses_proxy(url):
        # No proxies: given one, build_opener adds no handler of the environment's.
        handlers.append(urllib.request.ProxyHandler({}))
    opener = urllib.request.build_opener(*handlers)
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
    """At most ``ANSWER_MAX`` bytes of the body, as far as it came: its status
    decides, so a body that breaks or outlasts the deadline is no failure to retry."""
    body = bytearray()
    # A read at a time, so a failure loses none of what came before it.
    with contextlib.suppress(OSError, http.client.HTTPException):
        while len(body) < ANSWER_MAX:
            part = response.read1(ANSWER_MAX - len(body))
            if not part:
                break
            body += part
    return bytes(body)


def _text(value: Any) -> str:
    """An attribute's value as ingest writes it, a sequence as its JSON."""
    if isinstance(value, (list, tuple)):
        return json.dumps(
            list(value), ensure_ascii=False, separators=(",", ":"), default=str
        )
    return str(value)


def _past_caps(key: str, value: Any) -> bool:
    """Whether ingest drops an attribute for its length alone: a key past 128
    characters, or a value past 1,024 as ingest writes it."""
    return (
        len(key) > contract.ATTRIBUTE_KEY_MAX
        or len(_text(value)) > contract.ATTRIBUTE_VALUE_MAX
    )


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
        # Set by a shutdown past its deadline: no attempt follows, and a wait ends.
        self._given_up = threading.Event()
        self._sleep = sleep or self._wait_out
        self._url = f"{config.endpoint}/otlp/v1/traces"
        self._content_mode: str | None = None
        self._warned: set[str] = set()
        # The worker and the agent's threads warn alike: each key warns once.
        self._warned_lock = threading.Lock()
        self._resource = [
            _attribute(contract.DISTRO_NAME, contract.DISTRO),
            _attribute("telemetry.distro.version", __version__),
            _attribute("telemetry.sdk.language", "python"),
            _attribute("telemetry.sdk.name", "opentelemetry"),
            _attribute("telemetry.sdk.version", _sdk_version()),
        ]
        # The deployment, where the configuration names one: never from
        # OTEL_RESOURCE_ATTRIBUTES, which the application's exporters read.
        for key, value, most, what in (
            (
                contract.ENVIRONMENT,
                config.environment,
                contract.ENVIRONMENT_MAX,
                "environment",
            ),
            (contract.RELEASE, config.release, contract.RELEASE_MAX, "release"),
        ):
            if value is None:
                continue
            if len(value) > most:
                self.warn_once(
                    f"long:{key}",
                    f"telmoni: the {what} must be up to {most} characters; "
                    "it is not sent",
                    WarningInfo("input"),
                )
                continue
            self._resource.append(_attribute(key, value))

    @property
    def content_mode(self) -> str | None:
        """The project's mode, as the last answer named it; None before one."""
        return self._content_mode

    def give_up(self) -> None:
        """Gives up what is left: the wait before a retry ends now, and no attempt
        follows it. A shutdown past its deadline calls it."""
        self._given_up.set()

    def after_fork(self) -> None:
        """The child of a fork: its event and lock made anew, as a lock copied
        held would never be let go."""
        self._given_up = threading.Event()
        self._warned_lock = threading.Lock()

    def _wait_out(self, seconds: float) -> None:
        self._given_up.wait(seconds)

    def export(self, spans: Sequence[ReadableSpan]) -> bool:
        """Sends the spans, halved past ingest's caps; false once they failed for a
        reason not their own, which the rest of a batch they are part of would meet."""
        if not spans:
            return True
        # Ingest refuses an export past 1,000 spans whole, as one past 1 MiB below.
        if len(spans) > contract.BATCH_SPANS_MAX:
            return self._halve(spans)
        body = self._body(spans)
        if len(body) > contract.BATCH_MAX_BYTES:
            # Ingest refuses it whole, its 413 maybe breaking the upload still
            # going out, which reads as no answer and is sent again: halved.
            if len(spans) > 1:
                return self._halve(spans)
            # Content sent goes first, the longest field first, each named.
            cut = self._cut(spans[0])
            if cut is not None:
                self.warn_once(
                    "content-cut",
                    "telmoni: a span past the 1 MiB an export may hold was sent without "
                    "its longest content, named in telmoni.content_cut",
                    WarningInfo("trimmed"),
                )
                return self._send(cut)
            # A span alone past it goes without what ingest drops for its length,
            # and then fits, unless its own name does not.
            body = self._body(spans, trimmed=True)
            if len(body) > contract.BATCH_MAX_BYTES:
                self.warn_once(
                    "too-large",
                    "telmoni: a span past the 1 MiB an export may hold was dropped",
                    WarningInfo("too_large"),
                )
                return True
            self.warn_once(
                "trimmed",
                "telmoni: a span past the 1 MiB an export may hold was sent without "
                "the attributes ingest drops for their length",
                WarningInfo("trimmed"),
            )
        return self._send(body)

    def _cut(self, span: ReadableSpan) -> bytes | None:
        """The span's body without its longest content, as few fields as fit it in
        an export: a content field, or any past ingest's caps, which on a project
        that keeps content is content too; None where it sends no content, or fits
        without all of it."""
        if not self._keeps_content():
            return None
        content = sorted(
            (
                (key, len(_text(value)))
                for key, value in (span.attributes or {}).items()
                if key != contract.CONTENT_CUT
                and (key in contract.CONTENT or _past_caps(key, value))
            ),
            key=lambda field: -field[1],
        )
        cut: list[str] = []
        for key, _ in content:
            cut.append(key)
            body = self._body([span], cut=cut)
            if len(body) <= contract.BATCH_MAX_BYTES:
                return body
        return None

    def _keeps_content(self) -> bool:
        return self._config.capture_content and self._content_mode == "on"

    def _halve(self, spans: Sequence[ReadableSpan]) -> bool:
        """Exports each half of the spans, the second only where the first went."""
        half = len(spans) // 2
        return self.export(spans[:half]) and self.export(spans[half:])

    def _body(
        self,
        spans: Sequence[ReadableSpan],
        *,
        trimmed: bool = False,
        cut: Sequence[str] = (),
    ) -> bytes:
        # Python writes NaN and Infinity, which are no JSON: refused here, never sent.
        text = json.dumps(
            self.encode(spans, trimmed=trimmed, cut=cut),
            separators=(",", ":"),
            allow_nan=False,
            ensure_ascii=False,
        )
        # A lone surrogate is no text ingest reads, and would lose the batch.
        return replace_surrogates(text).encode()

    def _send(self, body: bytes) -> bool:
        headers = {
            "Content-Type": "application/json",
            "User-Agent": f"telmoni-python/{__version__}",
        }
        bearer = self._config.bearer()
        if bearer is not None:
            headers["Authorization"] = f"Bearer {bearer}"
        failure = ""
        # The last attempt's, as its words name it: none after a transport fault.
        last = WarningInfo("failed")
        for attempt in range(ATTEMPTS):
            if self._given_up.is_set():
                return False
            # This clock as the attempt leaves, the spans' own.
            sent = {**headers, contract.SENT_AT_HEADER: str(time.time_ns())}
            try:
                answer = self._transport(self._url, sent, body)
            except (OSError, http.client.HTTPException) as error:
                # No status came: a transport fault the next attempt may not meet.
                failure = type(error).__name__
                last = WarningInfo("failed")
                self._wait(None, attempt)
                continue
            if 200 <= answer.status < 300:
                self._read(answer)
                return True
            problem = _problem_type(answer.body)
            if answer.status in (429, 503):
                failure = f"{answer.status} {problem}"
                last = WarningInfo("failed", answer.status, problem or None)
                self._wait(answer.header("Retry-After"), attempt)
                continue
            self.warn_once(
                f"status:{answer.status}",
                f"telmoni: export refused with {answer.status} "
                f"{problem}; the batch was dropped",
                WarningInfo("refused", answer.status, problem or None),
            )
            # Refused for what it holds, the rest of a batch may still go.
            return answer.status in (400, 413)
        self.warn_once(
            f"failed:{failure}",
            f"telmoni: export failed after {ATTEMPTS} attempts ({failure}); "
            "the batch was dropped",
            last,
        )
        return False

    def encode(
        self,
        spans: Sequence[ReadableSpan],
        *,
        trimmed: bool = False,
        cut: Sequence[str] = (),
    ) -> dict[str, Any]:
        """OTLP's ``ExportTraceServiceRequest``, in its JSON encoding; ``trimmed``,
        without the attributes ingest drops for their length; ``cut``, without
        those content fields, named in ``telmoni.content_cut``."""
        return {
            "resourceSpans": [
                {
                    "resource": {"attributes": self._resource},
                    "scopeSpans": [
                        {
                            "scope": {"name": contract.SCOPE, "version": __version__},
                            "spans": [self._span(span, trimmed, cut) for span in spans],
                        }
                    ],
                }
            ]
        }

    def _span(
        self, span: ReadableSpan, trimmed: bool = False, cut: Sequence[str] = ()
    ) -> dict[str, Any]:
        keep_content = self._keeps_content()
        attributes = [
            _attribute(key, value)
            for key, value in (span.attributes or {}).items()
            if (keep_content or key not in contract.CONTENT)
            and key not in cut
            and not (cut and key == contract.CONTENT_CUT)
            and not (trimmed and _past_caps(key, value))
        ]
        if cut:
            attributes.append(_attribute(contract.CONTENT_CUT, list(cut)))
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
        # Trimmed of White_Space, as every SDK trims it: http.client keeps
        # trailing blanks.
        mode = _unset_if_blank(answer.header(contract.CONTENT_MODE_HEADER))
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
                self.warn_once(
                    f"dropped:{attribute}:{reason}", text, WarningInfo("dropped")
                )
                continue
            refused = _REFUSED.match(line)
            if refused:
                why = refused.group(2)
                self.warn_once(
                    f"refused:{why}",
                    f"telmoni: the server refused a span: {why}",
                    WarningInfo("span_refused"),
                )
                continue
            if line.startswith("and ") or not line:
                continue
            self.warn_once(
                f"line:{line}",
                f"telmoni: the server said: {line}",
                WarningInfo("said"),
            )

    def _wait(self, retry_after: str | None, attempt: int) -> None:
        """Before the next attempt; nothing after the last, or once given up."""
        if attempt + 1 >= ATTEMPTS or self._given_up.is_set():
            return
        given = _retry_seconds(retry_after)
        seconds = float(2**attempt) if given is None else given
        self._sleep(min(seconds, RETRY_AFTER_MAX_S))

    def warn_once(self, key: str, text: str, info: WarningInfo) -> None:
        """Log ``text`` once per client under ``key``, as every warning is."""
        with self._warned_lock:
            if key in self._warned:
                return
            # Bounded: a server naming ever-new attributes would otherwise grow
            # it for the life of the process; past the cap, warnings may repeat.
            if len(self._warned) >= WARNED_MAX:
                self._warned.clear()
            self._warned.add(key)
        # Outside the lock: the handler is the application's.
        warn(text, info=info)


def warn(text: str, *args: object, info: WarningInfo) -> None:
    """Log a warning, ``info`` as ``record.telmoni_warning``; a handler or filter
    that raises must not raise into the agent's own call, nor end the worker."""
    with contextlib.suppress(Exception):
        logger.warning(text, *args, extra={"telmoni_warning": info})


def _remedy(attribute: str, reason: str) -> str:
    if reason == "content":
        # The page, not a setting to change: which modes keep content is the
        # platform's to say, and only the organization's owner switches one.
        return f"the project's content mode decides whether content is kept: {CONTENT_DOCS}"
    if reason == "unlisted":
        return f"send it as the tag {contract.TAG_PREFIX}{attribute} to keep it"
    if reason == "over_cap":
        return "shorten it to keep it"
    if reason == "cardinality":
        return "keep tags to a few keys of a few values each: an id goes as the user or the session"
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


def _retry_seconds(header: str | None) -> float | None:
    """``Retry-After``'s whole seconds, read as text: a number of any length
    reaches the cap without converting, and one below zero is none."""
    given = _SECONDS.fullmatch(header or "")
    if given is None:
        return None
    sign, digits = given.groups()
    whole = digits.lstrip("0")
    if sign == "-":
        return 0.0
    return RETRY_AFTER_MAX_S if len(whole) > 2 else float(whole or "0")


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
        # An `(int, Enum)`'s own `str()` is its name, which ingest's parser refuses;
        # past 64 bits the number refuses the whole export there: as its text.
        whole = int(value)
        kind = "intValue" if -(1 << 63) <= whole < 1 << 63 else "stringValue"
        encoded = {kind: str(whole)}
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
