import contextlib
import http.client
import http.server
import logging
import os
import ssl
import threading
import time
from collections.abc import Iterator
from dataclasses import asdict
from typing import Any
from unittest import mock

import pytest
from opentelemetry.sdk.trace import ReadableSpan
from opentelemetry.trace import SpanContext

from conftest import FakeTransport, attributes_of, load_fixture
from telmoni import DEFAULT_ENDPOINT, Config, Secret, Telmoni, __version__, exporter
from telmoni.exporter import _urllib_transport


def test_the_default_endpoint_is_the_machine_host() -> None:
    assert DEFAULT_ENDPOINT == "https://api.telmoni.com"
    assert Config().endpoint == DEFAULT_ENDPOINT


def test_a_trailing_slash_is_stripped() -> None:
    assert (
        Config(endpoint="https://api.telmoni.local/").endpoint
        == "https://api.telmoni.local"
    )
    assert (
        Config(endpoint="https://api.telmoni.local///").endpoint
        == "https://api.telmoni.local"
    )


def test_a_blank_endpoint_organization_key_environment_or_release_reads_as_unset() -> (
    None
):
    config = Config(
        endpoint="  ", organization_id=" ", api_key="\t", environment=" ", release=""
    )
    assert config.endpoint == DEFAULT_ENDPOINT
    assert config.organization_id is None
    assert config.api_key is None
    assert config.bearer() is None
    assert config.environment is None
    assert config.release is None
    deployed = Config(environment=" staging ", release="2026.10.18")
    assert (deployed.environment, deployed.release) == ("staging", "2026.10.18")


def test_the_key_is_never_printed_or_serialized() -> None:
    config = Config(organization_id="org_1", api_key="telmoni_secret_key")
    for printed in (
        repr(config),
        str(config),
        repr(asdict(config)),
        str(asdict(config)),
    ):
        assert "telmoni_secret_key" not in printed
        assert "org_1" in printed
    assert isinstance(config.api_key, Secret)
    assert config.api_key.expose() == "telmoni_secret_key"
    assert config.bearer() == "telmoni_secret_key"
    assert Config(api_key=Secret("telmoni_x")).bearer() == "telmoni_x"


def test_from_env_reads_the_five_variables_and_no_other() -> None:
    with mock.patch.dict(os.environ, clear=False):
        os.environ.pop("TELMONI_ENDPOINT", None)
        os.environ["TELMONI_API_KEY"] = ""
        os.environ["TELMONI_ORG"] = "org_2"
        os.environ["TELMONI_ENVIRONMENT"] = "staging"
        os.environ["TELMONI_RELEASE"] = ""
        os.environ["OTEL_EXPORTER_OTLP_ENDPOINT"] = "https://elsewhere.example"
        os.environ["OTEL_RESOURCE_ATTRIBUTES"] = (
            "deployment.environment.name=leaked,service.version=leaked"
        )
        config = Config.from_env()
    assert config.endpoint == DEFAULT_ENDPOINT
    assert config.api_key is None
    assert config.organization_id == "org_2"
    assert config.environment == "staging"
    assert config.release is None
    with mock.patch.dict(os.environ, clear=False):
        os.environ.pop("TELMONI_ENVIRONMENT", None)
        os.environ["TELMONI_RELEASE"] = " 2026.10.18 "
        config = Config.from_env()
    assert config.environment is None
    assert config.release == "2026.10.18"


def test_sample_rate_is_above_zero_and_up_to_one() -> None:
    assert Config(sample_rate=1.0).sample_rate == 1.0
    assert Config(sample_rate=0.25).sample_rate == 0.25
    with pytest.raises(ValueError):
        Config(sample_rate=0)
    with pytest.raises(ValueError):
        Config(sample_rate=1.5)


def test_the_version_is_the_installed_one() -> None:
    from importlib import metadata

    assert __version__ == metadata.version("telmoni")


def test_the_client_builds_from_env_with_the_real_transport() -> None:
    with mock.patch.dict(os.environ, clear=False):
        os.environ.pop("TELMONI_ENDPOINT", None)
        client = Telmoni.from_env()
    try:
        assert client.config.endpoint == DEFAULT_ENDPOINT
        assert client.exporter._transport is _urllib_transport
    finally:
        client.shutdown()


_PROBLEM = b'{"message":"/errors/batch-too-large: the batch is too large"}'
"""A refusal's problem, which "/too-large" sends whole before it hangs up."""

_ANSWERS = {
    "/kept": (200, 2 << 20, b"x" * (2 << 20)),
    "/refused": (503, 2 << 20, b"x" * (2 << 20)),
    "/short": (200, 300, b"x" * 300),
    "/cut": (200, 300, b"x" * 100),
    "/too-large": (413, 300, _PROBLEM),
}
"""Each path's status, the length its answer declares, and what is sent."""

_SENT: list[bytes] = []
"""Every body the server was sent, in order."""


class _Answering(http.server.BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        _SENT.append(self.rfile.read(int(self.headers["Content-Length"])))
        # An endpoint's path, as the exporter's URL ends.
        self.path = self.path.removesuffix("/otlp/v1/traces")
        if self.path == "/chunked-cut":
            # One chunk and no last: http.client raises `IncompleteRead`.
            self.send_response(413)
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()
            self.wfile.write(b"%x\r\n%s\r\n" % (len(_PROBLEM), _PROBLEM))
            return
        if self.path in ("/slow-head", "/slow-body"):
            # A byte every 50 ms, each read's own timeout never reached.
            head = b"HTTP/1.1 200 OK\r\nContent-Length: 40\r\n\r\n"
            slow, fast = (
                (head, b"x" * 40) if self.path == "/slow-head" else (b"x" * 40, head)
            )
            with contextlib.suppress(OSError):
                if self.path == "/slow-body":
                    self.wfile.write(fast)
                for byte in slow:
                    self.wfile.write(bytes([byte]))
                    time.sleep(0.05)
                if self.path == "/slow-head":
                    self.wfile.write(fast)
            return
        status, declared, sent = _ANSWERS[self.path]
        self.send_response(status)
        self.send_header("Content-Length", str(declared))
        self.end_headers()
        with contextlib.suppress(OSError):  # the client hangs up past its cap
            self.wfile.write(sent)

    def log_message(self, format: str, *args: Any) -> None:
        pass


@pytest.fixture
def answering() -> Iterator[str]:
    _SENT.clear()
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Answering)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        server.server_close()


def test_an_answer_is_read_up_to_its_first_mebibyte(answering: str) -> None:
    for path, status, read in (
        ("/kept", 200, 1 << 20),
        ("/refused", 503, 1 << 20),
        ("/short", 200, 300),
    ):
        answer = _urllib_transport(answering + path, {}, b"{}")
        assert (answer.status, len(answer.body)) == (status, read)


def test_an_answer_whose_body_breaks_is_kept_as_far_as_it_came(
    answering: str,
) -> None:
    """Its status decides: a body cut short is no failure to retry."""
    for path, status, body in (
        ("/cut", 200, b"x" * 100),
        ("/too-large", 413, _PROBLEM),
        ("/chunked-cut", 413, _PROBLEM),
    ):
        answer = _urllib_transport(answering + path, {}, b"{}")
        assert (answer.status, answer.body) == (status, body)


def test_a_refusal_whose_body_breaks_is_sent_once(
    answering: str, sleeps: list[float], caplog: pytest.LogCaptureFixture
) -> None:
    client = Telmoni(Config(endpoint=answering + "/too-large", api_key="k"))
    client.exporter._sleep = sleeps.append
    try:
        with client.run("bot"):
            pass
    finally:
        client.shutdown()
    assert _SENT, "nothing was sent"
    assert len(set(_SENT)) == len(_SENT), "an export was sent again"
    assert sleeps == []
    warned = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warned == [
        "telmoni: export refused with 413 /errors/batch-too-large; the batch was dropped"
    ]


def test_an_answer_whose_head_comes_slowly_is_given_up_at_the_requests_deadline(
    answering: str,
) -> None:
    started = time.monotonic()
    with (
        mock.patch("telmoni.exporter.REQUEST_TIMEOUT_S", 0.5),
        pytest.raises(TimeoutError),
    ):
        _urllib_transport(answering + "/slow-head", {}, b"{}")
    assert time.monotonic() - started < 1.5


_TAKEN: list[str] = []
"""Every request the proxy was given, in order."""


class _Proxy(http.server.BaseHTTPRequestHandler):
    """A proxy that takes every request it is given and answers it itself."""

    def do_POST(self) -> None:
        _TAKEN.append(self.path)
        self.send_response(200)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"{}")

    def log_message(self, format: str, *args: Any) -> None:
        pass


def test_this_machine_is_reached_past_any_proxy_the_environment_names(
    answering: str,
) -> None:
    """A proxy would take plain HTTP's key off this machine in the clear."""
    _TAKEN.clear()
    proxy = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Proxy)
    threading.Thread(target=proxy.serve_forever, daemon=True).start()
    named = f"http://127.0.0.1:{proxy.server_port}"
    port = answering.rsplit(":", 1)[1]
    try:
        with mock.patch.dict(
            os.environ,
            {
                "HTTP_PROXY": named,
                "http_proxy": named,
                "ALL_PROXY": named,
                "NO_PROXY": "",
                "no_proxy": "",
            },
        ):
            for host in ("127.0.0.1", "localhost", "LOCALHOST"):
                answer = _urllib_transport(f"http://{host}:{port}/short", {}, b"{}")
                assert answer.status == 200 and len(answer.body) == 300, host
            assert _TAKEN == []
            # Another host still goes through it, as urllib sends.
            _urllib_transport("http://telmoni-test.invalid/otlp/v1/traces", {}, b"{}")
            assert _TAKEN == ["http://telmoni-test.invalid/otlp/v1/traces"]
    finally:
        proxy.shutdown()
        proxy.server_close()


def _spans(*sizes: int, char: str = "x") -> list[ReadableSpan]:
    """A span named ``s<n>`` for each size, an attribute of that many characters
    beside one of a few."""
    context = SpanContext(trace_id=1, span_id=1, is_remote=False)
    return [
        ReadableSpan(
            name=f"s{n}",
            context=context,
            start_time=1,
            end_time=2,
            attributes={"telmoni.tag.big": char * size, "app.kept": "yes"},
        )
        for n, size in enumerate(sizes)
    ]


def test_an_export_past_ingest_s_1_mib_is_halved_until_each_part_fits() -> None:
    """Ingest refuses it whole, its 413 maybe breaking an upload still going out."""
    transport = FakeTransport()
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        client.exporter.export(_spans(*[300_000] * 8))
    finally:
        client.shutdown()
    sizes = [len(body) for _, _, body in transport.requests]
    assert len(sizes) == 4 and max(sizes) <= 1 << 20, sizes
    assert [s["name"] for s in transport.spans()] == [f"s{n}" for n in range(8)]


def test_the_mebibyte_is_counted_in_the_bytes_sent() -> None:
    """Two bytes a character: past 1 MiB in bytes, short of it in characters."""
    transport = FakeTransport()
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        client.exporter.export(_spans(*[200_000] * 4, char="é"))
    finally:
        client.shutdown()
    sizes = [len(body) for _, _, body in transport.requests]
    assert len(sizes) == 2 and max(sizes) <= 1 << 20, sizes


def test_a_span_alone_past_it_goes_without_what_ingest_drops_for_its_length(
    caplog: pytest.LogCaptureFixture,
) -> None:
    """Rather than lost, since a run's own span carries the run's end."""
    transport = FakeTransport()
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        client.exporter.export(_spans(100, 1 << 20, 100))
        client.exporter.export(_spans(1 << 20))
    finally:
        client.shutdown()
    kept = [(s["name"], sorted(attributes_of(s))) for s in transport.spans()]
    both = ["app.kept", "telmoni.tag.big"]
    assert kept == [
        ("s0", both),
        ("s1", ["app.kept"]),
        ("s2", both),
        ("s0", ["app.kept"]),
    ]
    warned = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warned == [
        (
            "telmoni: a span past the 1 MiB an export may hold was sent without "
            "the attributes ingest drops for their length"
        )
    ]


def test_a_span_whose_own_name_passes_it_is_dropped_with_one_warning(
    caplog: pytest.LogCaptureFixture,
) -> None:
    transport = FakeTransport()
    client = Telmoni(Config(api_key="k"), transport=transport)
    context = SpanContext(trace_id=1, span_id=1, is_remote=False)
    named = ReadableSpan(
        name="x" * (1 << 20), context=context, start_time=1, end_time=2
    )
    try:
        client.exporter.export([named])
    finally:
        client.shutdown()
    assert transport.requests == []
    warned = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert warned == ["telmoni: a span past the 1 MiB an export may hold was dropped"]


def test_an_answer_whose_body_comes_slowly_is_kept_as_far_as_it_came_by_the_deadline(
    answering: str,
) -> None:
    started = time.monotonic()
    with mock.patch("telmoni.exporter.REQUEST_TIMEOUT_S", 0.5):
        answer = _urllib_transport(answering + "/slow-body", {}, b"{}")
    assert time.monotonic() - started < 1.5
    assert answer.status == 200
    assert len(answer.body) < 40 and set(answer.body) <= {ord("x")}


def test_the_transport_makes_one_tls_context_through_the_hook_urllib_reads(
    answering: str,
) -> None:
    """So an application's trust set there holds, its certificates loaded once."""
    made: list[None] = []
    real = ssl.create_default_context

    def counted(*args: Any, **kwargs: Any) -> ssl.SSLContext:
        made.append(None)
        return real(*args, **kwargs)

    exporter._tls.cache_clear()
    with mock.patch("ssl._create_default_https_context", counted):
        for _ in range(2):
            _urllib_transport(answering + "/short", {}, b"{}")
    exporter._tls.cache_clear()
    assert len(made) == 1


def test_a_span_time_past_what_ingest_reads_goes_as_0() -> None:
    """So ingest refuses that span alone, where as is it would refuse the export."""
    transport = FakeTransport()
    client = Telmoni(Config(api_key="k"), transport=transport)
    context = SpanContext(trace_id=1, span_id=1, is_remote=False)
    times = [(2**64 - 1, 2**64 - 1), (2**64, -1)]
    try:
        client.exporter.export(
            [
                ReadableSpan(
                    name=f"s{n}", context=context, start_time=start, end_time=end
                )
                for n, (start, end) in enumerate(times)
            ]
        )
    finally:
        client.shutdown()
    sent = {
        s["name"]: (s["startTimeUnixNano"], s["endTimeUnixNano"])
        for s in transport.spans()
    }
    assert sent == {"s0": (str(2**64 - 1), str(2**64 - 1)), "s1": ("0", "0")}


def test_every_endpoint_is_read_as_the_fixture_reads_it() -> None:
    """The four SDKs take and refuse the same endpoints (``endpoints.json``)."""
    fixture = load_fixture("endpoints.json")
    for case in fixture["taken"]:
        assert Config(endpoint=case["given"]).endpoint == case["endpoint"], case
        assert Config(endpoint=case["endpoint"]).endpoint == case["endpoint"], case
    for case in fixture["refused"]:
        why = "unencrypted HTTP" if case["as"] == "cleartext" else "http:// or https://"
        with pytest.raises(ValueError, match=why):
            Config(endpoint=case["given"])


def test_a_configuration_changed_after_it_was_built_is_checked_again() -> None:
    # The client checks what it is given, as the other SDKs do: a field set
    # after the configuration was built skipped the checks it was built with.
    config = Config(api_key="k")
    config.endpoint = "http://api.example.com"
    with pytest.raises(ValueError, match="unencrypted HTTP"):
        Telmoni(config)
    config.endpoint = "https://api.telmoni.local//"
    client = Telmoni(config)
    try:
        assert client.config.endpoint == "https://api.telmoni.local"
        assert client.exporter._url == "https://api.telmoni.local/otlp/v1/traces"
    finally:
        client.shutdown()


def test_a_secret_is_never_pickled_but_copies_as_itself() -> None:
    import copy
    import pickle

    secret = Secret("telmoni_x")
    assert copy.copy(secret) is secret and copy.deepcopy(secret) is secret
    with pytest.raises(TypeError):
        pickle.dumps(secret)
    with pytest.raises(TypeError):
        pickle.dumps(Config(api_key="telmoni_x"))
