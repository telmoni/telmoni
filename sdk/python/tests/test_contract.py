import contextlib
import http.client
import http.server
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

from conftest import FakeTransport
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


_ANSWERS = {
    "/kept": (200, 2 << 20, 2 << 20),
    "/refused": (503, 2 << 20, 2 << 20),
    "/short": (200, 300, 300),
    "/cut": (200, 300, 100),
}
"""Each path's status, the length its answer declares, and how much is sent."""


class _Answering(http.server.BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        self.rfile.read(int(self.headers["Content-Length"]))
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
            self.wfile.write(b"x" * sent)

    def log_message(self, format: str, *args: Any) -> None:
        pass


@pytest.fixture
def answering() -> Iterator[str]:
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


def test_an_answer_cut_short_is_still_a_broken_one(answering: str) -> None:
    with pytest.raises(http.client.IncompleteRead):
        _urllib_transport(answering + "/cut", {}, b"{}")


@pytest.mark.parametrize("path", ["/slow-head", "/slow-body"])
def test_an_answer_sent_slowly_is_given_up_at_the_requests_deadline(
    answering: str, path: str
) -> None:
    started = time.monotonic()
    with (
        mock.patch("telmoni.exporter.REQUEST_TIMEOUT_S", 0.5),
        pytest.raises(TimeoutError),
    ):
        _urllib_transport(answering + path, {}, b"{}")
    assert time.monotonic() - started < 1.5


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


def test_plain_http_is_refused_except_to_this_machine() -> None:
    for endpoint in (
        "http://localhost:8080",
        "http://api.localhost",
        "http://127.0.0.1:8080/",
        "http://[::1]:8080",
        "https://api.example.com",
    ):
        assert Config(endpoint=endpoint).endpoint == endpoint.rstrip("/")
    for endpoint in ("http://api.example.com", "http://10.0.0.5:8080"):
        with pytest.raises(ValueError, match="unencrypted HTTP"):
            Config(endpoint=endpoint)


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
