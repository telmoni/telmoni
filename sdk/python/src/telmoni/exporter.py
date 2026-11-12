"""The exporter: OTLP's JSON encoding over the standard library.

It takes content off every span unless the code asked and the project's mode
allows, puts each image or file a message carries to
``PUT {endpoint}/v1/media/{sha256}`` where it sends content, scrubs the rest of
that content of secrets, and of personal data where asked, sends to
``POST {endpoint}/otlp/v1/traces``, posts the late scores the batch carries
to ``POST {endpoint}/v1/scores``, waits out a ``Retry-After``, and reads ``Telmoni-Content-Mode`` and the partial success
from each answer, warning once for each attribute an answer names dropped.
"""

from __future__ import annotations

import base64
import binascii
import contextlib
import functools
import hashlib
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

from opentelemetry.sdk.trace import Event, ReadableSpan
from opentelemetry.trace import SpanKind, Status, StatusCode

from . import contract, scrub
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
_MASKED_OUT = object()
"""What the mask left a field: its span is sent without it."""
_RETRIED = frozenset({429, 502, 503, 504})
"""OTLP's retryable statuses: a 502 or a 504 is a load balancer's, a server's
rollout among its causes."""
REQUEST_TIMEOUT_S = 10.0
ANSWER_MAX = 1 << 20
"""How much of an answer is read: a problem's JSON is small, and an export
never pays for more."""
RETRY_AFTER_MAX_S = 60.0
WARNED_MAX = 1024
"""How many distinct warnings are remembered before the set starts over."""
UPLOADED_MAX = 4096
"""How many files put are remembered before the set starts over: an image a
conversation repeats in every call is put once."""
CONTENT_DOCS = (
    "https://github.com/telmoni/telmoni/blob/main/web/content/docs/api/ingest.mdx"
    "#what-is-dropped-at-the-door"
)
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


Transport = Callable[[str, str, Mapping[str, str], bytes], Answer]
"""Sends one request, ``(method, url, headers, body)``; raises ``OSError`` when
the platform could not be reached."""


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


def _urllib_transport(
    method: str, url: str, headers: Mapping[str, str], body: bytes
) -> Answer:
    request = urllib.request.Request(
        url, data=body, headers=dict(headers), method=method
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


_Lifted = dict[int, dict[str, str]]
"""Content fields with their blobs lifted out, by the ``id`` of their span."""


def _attributes(span: ReadableSpan, lifted: _Lifted) -> Mapping[str, Any]:
    """The span's attributes, any of its fields lifted in their place."""
    own = span.attributes or {}
    fields = lifted.get(id(span))
    return {**own, **fields} if fields else own


def _handed(value: Any) -> str:
    """A field as the mask and the scrubbers read it: a string, any other value as
    its JSON."""
    if isinstance(value, str):
        return value
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False, default=str)


def _answered(
    span: ReadableSpan, attributes: Mapping[str, Any], answer: Callable[[str, Any], Any]
) -> tuple[dict[str, Any], list[Event], Status]:
    """Each content field of ``attributes``, each event attribute but a score's
    own and the status message, as ``answer`` gives it, by its key; one it answers
    ``_MASKED_OUT`` is left off."""
    kept: dict[str, Any] = {}
    for key, value in attributes.items():
        if key in contract.CONTENT and key != contract.CONTENT_CUT:
            value = answer(key, value)
            if value is _MASKED_OUT:
                continue
        kept[key] = value
    events = []
    for event in span.events:
        fields: dict[str, Any] = {}
        for key, value in (event.attributes or {}).items():
            if not _is_score_key(event, key):
                value = answer(key, value)
            if value is not _MASKED_OUT:
                fields[key] = value
        events.append(Event(event.name, fields, event.timestamp))
    status = span.status
    if status.description:
        description = answer(contract.STATUS_DESCRIPTION, status.description)
        status = Status(
            status.status_code,
            None if description is _MASKED_OUT else description,
        )
    return kept, events, status


def _rebuilt(
    span: ReadableSpan,
    attributes: dict[str, Any],
    events: list[Event],
    status: Status,
) -> ReadableSpan:
    return ReadableSpan(
        name=span.name,
        context=span.context,
        parent=span.parent,
        resource=span.resource,
        attributes=attributes,
        events=events,
        links=span.links,
        kind=span.kind,
        status=status,
        start_time=span.start_time,
        end_time=span.end_time,
        instrumentation_scope=span.instrumentation_scope,
    )


def _header_safe(value: Any) -> bool:
    """Whether a part's type can go as a header: printable ASCII, not blank."""
    return (
        isinstance(value, str)
        and value.strip(" ") != ""
        and all(" " <= c <= "~" for c in value)
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
        self._media_url = f"{config.endpoint}{contract.MEDIA_PATH}"
        self._scores_url = f"{config.endpoint}{contract.SCORES_PATH}"
        # Only the worker exports, so the set needs no lock.
        self._uploaded: set[str] = set()
        self._content_mode: str | None = None
        # Held here so a scrubber that fails can stand in for it.
        self._scrubber: Callable[[str, bool, bool], tuple[str, dict[str, int]]] = (
            scrub.scrub
        )
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
        """The project's mode, as the last answer named it; None while unheard."""
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
        # Late scores first, never spans: no mask, lift or ask sees them.
        sent = [text for span in spans if (text := _score_sent(span)) is not None]
        if sent:
            spans = [s for s in spans if _score_sent(s) is None]
            self._send_scores(sent)
        if not spans:
            return True
        # Yet to hear the project's mode, a client asks it first, so a process
        # that sends once, as a hook does, still sends its content.
        if (
            self._config.capture_content
            and self._content_mode is None
            and any(_holds_content(span) for span in spans)
        ):
            self._ask()
        # Decided once, so no answer met halfway sends content the mask never saw.
        keep = self._keeps_content()
        if keep:
            spans = self._mask(spans)
        lifted = self._lift(spans) if keep else {}
        if keep:
            spans, lifted = self._scrub(spans, lifted)
        return self._export(spans, lifted, keep)

    def _export(
        self, spans: Sequence[ReadableSpan], lifted: _Lifted, keep: bool
    ) -> bool:
        # Ingest refuses an export past 1,000 spans whole, as one past 1 MiB below.
        if len(spans) > contract.BATCH_SPANS_MAX:
            return self._halve(spans, lifted, keep)
        body = self._body(spans, lifted, keep)
        if len(body) > contract.BATCH_MAX_BYTES:
            # Ingest refuses it whole, its 413 maybe breaking the upload still
            # going out, which reads as no answer and is sent again: halved.
            if len(spans) > 1:
                return self._halve(spans, lifted, keep)
            # Content sent goes first, the longest field first, each named.
            cut = self._cut(spans[0], lifted, keep)
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
            body = self._body(spans, lifted, keep, trimmed=True)
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

    def _cut(self, span: ReadableSpan, lifted: _Lifted, keep: bool) -> bytes | None:
        """The span's body without its longest content, as few fields as fit it in
        an export: a content field, or any past ingest's caps, which on a project
        that keeps content is content too; None where it sends no content, or fits
        without all of it."""
        if not keep:
            return None
        content = sorted(
            (
                (key, len(_text(value)))
                for key, value in _attributes(span, lifted).items()
                if key != contract.CONTENT_CUT
                and (key in contract.CONTENT or _past_caps(key, value))
            ),
            key=lambda field: -field[1],
        )
        cut: list[str] = []
        for key, _ in content:
            cut.append(key)
            body = self._body([span], lifted, keep, cut=cut)
            if len(body) <= contract.BATCH_MAX_BYTES:
                return body
        return None

    def _keeps_content(self) -> bool:
        return self._config.capture_content and self._content_mode == "on"

    def _halve(
        self, spans: Sequence[ReadableSpan], lifted: _Lifted, keep: bool
    ) -> bool:
        """Exports each half of the spans, the second only where the first went."""
        half = len(spans) // 2
        return self._export(spans[:half], lifted, keep) and self._export(
            spans[half:], lifted, keep
        )

    def _mask(self, spans: Sequence[ReadableSpan]) -> Sequence[ReadableSpan]:
        """The spans with every content field, event attribute and status message
        as the code's mask answers it, before a part it takes out could be put."""
        if self._config.mask is None:
            return spans
        return [
            _rebuilt(span, *_answered(span, span.attributes or {}, self._masked))
            for span in spans
        ]

    def _scrub(
        self, spans: Sequence[ReadableSpan], lifted: _Lifted
    ) -> tuple[Sequence[ReadableSpan], _Lifted]:
        """The spans with the same fields the mask is handed scrubbed as they are
        sent, their images lifted out first, so no base64 is read as text, and
        what was taken out counted in ``telmoni.scrubbed``."""
        # A None set since the client was built reads as the defaults.
        secrets = self._config.scrub_secrets is not False
        pii = self._config.scrub_pii is True
        if not (secrets or pii):
            return spans, lifted
        scrubbed = []
        for span in spans:
            counts: dict[str, int] = {}
            answer = functools.partial(
                self._scrubbed, secrets=secrets, pii=pii, counts=counts
            )
            attributes, events, status = _answered(
                span, _attributes(span, lifted), answer
            )
            if counts:
                attributes[contract.SCRUBBED] = json.dumps(
                    counts, sort_keys=True, separators=(",", ":")
                )
            scrubbed.append(_rebuilt(span, attributes, events, status))
        # Each lifted field is in its span now, scrubbed.
        return scrubbed, {}

    def _scrubbed(
        self, key: str, value: Any, *, secrets: bool, pii: bool, counts: dict[str, int]
    ) -> Any:
        """One field with each match replaced, as text where one was, else as it
        was; ``_MASKED_OUT`` where the scrubber fails on it, counted, since a field
        is never sent unscrubbed."""
        try:
            text, found = self._scrubber(_handed(value), secrets, pii)
        except Exception:  # noqa: BLE001 - a scrubber's failure never costs the batch
            counts[f"dropped:{key}"] = counts.get(f"dropped:{key}", 0) + 1
            return _MASKED_OUT
        for kind, count in found.items():
            counts[kind] = counts.get(kind, 0) + count
        return text if found else value

    def _masked(self, key: str, value: Any) -> Any:
        """One field as the mask answers it, handed as a string, any other value as
        its JSON; ``_MASKED_OUT`` to send the span without it. An answer unchanged
        keeps the value as it was."""
        given = _handed(value)
        mask = cast("Callable[[str, str], object]", self._config.mask)
        try:
            answer = mask(key, given)
        except Exception:  # noqa: BLE001 - the code's mask never costs the batch
            answer = _MASKED_OUT
        if answer is None:
            return _MASKED_OUT
        if not isinstance(answer, str):
            # The key alone: an error's words may hold the content.
            self.warn_once(
                f"mask:{key}",
                f"telmoni: the mask failed on {key}; the span was sent without it",
                WarningInfo("mask_failed"),
            )
            return _MASKED_OUT
        return value if answer == given else answer

    def _lift(self, spans: Sequence[ReadableSpan]) -> _Lifted:
        """Each span's messages with every ``blob`` part lifted out, by the span's
        ``id``: a blob repeated in every span would bloat each, past 1 MiB maybe."""
        lifted: _Lifted = {}
        for span in spans:
            for key, value in (span.attributes or {}).items():
                if key not in contract.MEDIA_FIELDS or not isinstance(value, str):
                    continue
                # Nesting past Python's stack is left as it is, never the batch's loss.
                with contextlib.suppress(ValueError, RecursionError):
                    decoded = json.loads(value)
                    if isinstance(decoded, list) and self._lift_parts(decoded):
                        lifted.setdefault(id(span), {})[key] = json.dumps(
                            decoded, separators=(",", ":"), ensure_ascii=False
                        )
        return lifted

    def _lift_parts(self, parts: list[Any]) -> bool:
        """Replaces each ``blob`` part in place, a message's parts too; whether any was."""
        lifted = False
        for at, part in enumerate(parts):
            if isinstance(part, dict) and isinstance(part.get("parts"), list):
                lifted = self._lift_parts(part["parts"]) or lifted
                continue
            file = self._file_part(part)
            if file is not None:
                parts[at] = file
                lifted = True
        return lifted

    def _file_part(self, part: Any) -> dict[str, Any] | None:
        """The ``file`` part naming a ``blob`` part's bytes, put first unless past
        the platform's cap or put already; None for any other part."""
        if not isinstance(part, dict) or part.get("type") != "blob":
            return None
        content = part.get("content")
        if not isinstance(content, str):
            return None
        try:
            data = base64.b64decode(content, validate=True)
        except (binascii.Error, ValueError):
            return None
        sha256 = hashlib.sha256(data).hexdigest()
        if len(data) <= contract.MEDIA_MAX_BYTES and sha256 not in self._uploaded:
            self._upload(sha256, data, part.get("mime_type"))
        # Named whatever became of it: the platform says itself what it holds.
        file: dict[str, Any] = {"type": "file"}
        for name in ("modality", "mime_type"):
            if name in part:
                file[name] = part[name]
        file["file_id"] = f"{contract.FILE_ID_PREFIX}{sha256}"
        return file

    def _upload(self, sha256: str, data: bytes, mime_type: Any) -> None:
        """Puts one file, once: a failure is warned of, never the export's."""
        if self._given_up.is_set():
            return
        headers = self._headers(
            mime_type if _header_safe(mime_type) else contract.MEDIA_DEFAULT_TYPE
        )
        info = WarningInfo("upload_failed")
        try:
            answer = self._transport("PUT", self._media_url + sha256, headers, data)
        except Exception as error:  # noqa: BLE001 - an upload never costs the batch
            failure = type(error).__name__
        else:
            if 200 <= answer.status < 300:
                if len(self._uploaded) >= UPLOADED_MAX:
                    self._uploaded.clear()
                self._uploaded.add(sha256)
                return
            failure = str(answer.status)
            info = WarningInfo("upload_failed", answer.status)
        self.warn_once(
            f"upload:{failure}",
            f"telmoni: an image or file in a message could not be uploaded "
            f"({failure}); its part was sent naming it, which the platform "
            "may not hold",
            info,
        )

    def _body(
        self,
        spans: Sequence[ReadableSpan],
        lifted: _Lifted,
        keep: bool,
        *,
        trimmed: bool = False,
        cut: Sequence[str] = (),
    ) -> bytes:
        # Python writes NaN and Infinity, which are no JSON: refused here, never sent.
        text = json.dumps(
            self.encode(spans, lifted, keep, trimmed=trimmed, cut=cut),
            separators=(",", ":"),
            allow_nan=False,
            ensure_ascii=False,
        )
        # A lone surrogate is no text ingest reads, and would lose the batch.
        return replace_surrogates(text).encode()

    def _headers(self, content_type: str = "application/json") -> dict[str, str]:
        headers = {
            "Content-Type": content_type,
            "User-Agent": f"telmoni-python/{__version__}",
        }
        bearer = self._config.bearer()
        if bearer is not None:
            headers["Authorization"] = f"Bearer {bearer}"
        return headers

    def _ask(self) -> None:
        """Asks the project's content mode with an export of no spans, which
        ingest answers as the mode is now: once and silently, since the export
        after it retries and warns of what it meets, its content held back
        meanwhile."""
        if self._given_up.is_set():
            return
        sent = {**self._headers(), contract.SENT_AT_HEADER: str(time.time_ns())}
        try:
            answer = self._transport("POST", self._url, sent, _ASK)
        except (OSError, http.client.HTTPException):
            # Unheard, the mode keeps the content back, as no answer does.
            return
        if 200 <= answer.status < 300:
            self._read(answer)

    def _send(self, body: bytes) -> bool:
        answer = self._post(self._url, body, "export", "the batch was dropped")
        if answer is None:
            return False
        if 200 <= answer.status < 300:
            self._read(answer)
            return True
        # Refused for what it holds, the rest of a batch may still go.
        return answer.status in (400, 413)

    def _send_scores(self, sent: Sequence[str]) -> None:
        """Posts each item, in order, in bodies of up to ``SCORES_SENT_MAX``."""
        for at in range(0, len(sent), contract.SCORES_SENT_MAX):
            items = ",".join(sent[at : at + contract.SCORES_SENT_MAX])
            body = replace_surrogates(f'{{"scores":[{items}]}}').encode()
            self._post(self._scores_url, body, "scores", "they were dropped")

    def _post(self, url: str, body: bytes, what: str, lost: str) -> Answer | None:
        """``body`` posted up to ``ATTEMPTS`` times, each retried status waited
        out: the answer that ended it, a 2xx or a refusal warned of, else None
        once given up or failed, warned of in ``what``'s and ``lost``'s words."""
        headers = self._headers()
        failure = ""
        # The last attempt's, as its words name it: none after a transport fault.
        last = WarningInfo("failed")
        for attempt in range(ATTEMPTS):
            if self._given_up.is_set():
                return None
            # This clock as the attempt leaves, the spans' own.
            sent = {**headers, contract.SENT_AT_HEADER: str(time.time_ns())}
            try:
                answer = self._transport("POST", url, sent, body)
            except (OSError, http.client.HTTPException) as error:
                # No status came: a transport fault the next attempt may not meet.
                failure = type(error).__name__
                last = WarningInfo("failed")
                self._wait(None, attempt)
                continue
            if 200 <= answer.status < 300:
                return answer
            problem = _problem_type(answer.body)
            if answer.status in _RETRIED:
                failure = _named(answer.status, problem)
                last = WarningInfo("failed", answer.status, problem or None)
                self._wait(answer.header("Retry-After"), attempt)
                continue
            self.warn_once(
                f"{what}:status:{answer.status}",
                f"telmoni: {what} refused with {_named(answer.status, problem)}; "
                f"{lost}",
                WarningInfo("refused", answer.status, problem or None),
            )
            return answer
        self.warn_once(
            f"{what}:failed:{failure}",
            f"telmoni: {what} failed after {ATTEMPTS} attempts ({failure}); {lost}",
            last,
        )
        return None

    def encode(
        self,
        spans: Sequence[ReadableSpan],
        lifted: _Lifted,
        keep: bool,
        *,
        trimmed: bool = False,
        cut: Sequence[str] = (),
    ) -> dict[str, Any]:
        """OTLP's ``ExportTraceServiceRequest``, in its JSON encoding, ``lifted``'s
        fields in place of the spans' own; ``keep``, with their content;
        ``trimmed``, without the attributes
        ingest drops for their length; ``cut``, without those content fields,
        named in ``telmoni.content_cut``."""
        return {
            "resourceSpans": [
                {
                    "resource": {"attributes": self._resource},
                    "scopeSpans": [
                        {
                            "scope": {"name": contract.SCOPE, "version": __version__},
                            "spans": [
                                self._span(span, lifted, keep, trimmed, cut)
                                for span in spans
                            ],
                        }
                    ],
                }
            ]
        }

    def _span(
        self,
        span: ReadableSpan,
        lifted: _Lifted,
        keep_content: bool,
        trimmed: bool = False,
        cut: Sequence[str] = (),
    ) -> dict[str, Any]:
        attributes = [
            _attribute(key, value)
            for key, value in _attributes(span, lifted).items()
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
        # The application's span current as the trace opened, if any.
        if span.links:
            encoded["links"] = [
                {
                    "traceId": format(link.context.trace_id, "032x"),
                    "spanId": format(link.context.span_id, "016x"),
                }
                for link in span.links
            ]
        # Events and the status message carry an exception's message and stack
        # trace: content, and the first to go from a span trimmed to fit.
        events = keep_content and not trimmed
        # A score's own keys are no content: its event goes with them alone.
        sent = [
            {
                "timeUnixNano": _nanos(event.timestamp),
                "name": event.name,
                "attributes": [
                    _attribute(key, value)
                    for key, value in (event.attributes or {}).items()
                    if events or _is_score_key(event, key)
                ],
            }
            for event in span.events
            if events or event.name == contract.EVALUATION_EVENT
        ]
        if sent:
            encoded["events"] = sent
        if span.status.status_code is StatusCode.ERROR:
            encoded["status"] = {"code": 2}
            if events and span.status.description:
                encoded["status"]["message"] = span.status.description
        return encoded

    def _read(self, answer: Answer) -> None:
        # Trimmed of White_Space, as every SDK trims it: http.client keeps
        # trailing blanks.
        mode = _unset_if_blank(answer.header(contract.CONTENT_MODE_HEADER))
        if mode:
            # A word ingest does not send reads as none, which keeps content home.
            self._content_mode = mode if mode in ("off", "on") else None
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


_ASK = b'{"resourceSpans":[]}'
"""An export of no spans: how a client asks the project's content mode."""


def _holds_content(span: ReadableSpan) -> bool:
    """Whether a span holds anything the content lock decides on: a content
    field, an event's attribute but a score's own, or a status message."""
    return (
        any(key in contract.CONTENT for key in span.attributes or {})
        or any(
            not _is_score_key(event, key)
            for event in span.events
            for key in (event.attributes or {}) or ("",)
        )
        or bool(span.status.description)
    )


def _is_score_key(event: Event, key: str) -> bool:
    """Whether ``key`` is a score's own on an evaluation event, which ingest
    reads the score from and which leaves under no lock."""
    return event.name == contract.EVALUATION_EVENT and key in contract.SCORE_KEYS


def _named(status: int, problem: str) -> str:
    """An answer as a warning names it: its status, then the problem type its
    body named, where it named one."""
    return f"{status} {problem}" if problem else str(status)


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


def _score_sent(span: ReadableSpan) -> str | None:
    """The ``/v1/scores`` item a late score's span carries: the client's own
    root span, so no application's attribute diverts a span."""
    if span.name != contract.SCORE_SPAN or span.parent is not None:
        return None
    text = (span.attributes or {}).get(contract.SCORE_SENT)
    return text if isinstance(text, str) else None
