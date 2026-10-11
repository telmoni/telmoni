import os
from dataclasses import asdict
from unittest import mock

import pytest

from telmoni import DEFAULT_ENDPOINT, Config, Secret, Telmoni, __version__
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


def test_a_blank_endpoint_organization_or_key_reads_as_unset() -> None:
    config = Config(endpoint="  ", organization_id=" ", api_key="\t")
    assert config.endpoint == DEFAULT_ENDPOINT
    assert config.organization_id is None
    assert config.api_key is None
    assert config.bearer() is None


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


def test_from_env_reads_the_three_variables_and_no_other() -> None:
    with mock.patch.dict(os.environ, clear=False):
        os.environ.pop("TELMONI_ENDPOINT", None)
        os.environ["TELMONI_API_KEY"] = ""
        os.environ["TELMONI_ORG"] = "org_2"
        os.environ["OTEL_EXPORTER_OTLP_ENDPOINT"] = "https://elsewhere.example"
        config = Config.from_env()
    assert config.endpoint == DEFAULT_ENDPOINT
    assert config.api_key is None
    assert config.organization_id == "org_2"


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


def test_a_secret_is_never_pickled_but_copies_as_itself() -> None:
    import copy
    import pickle

    secret = Secret("telmoni_x")
    assert copy.copy(secret) is secret and copy.deepcopy(secret) is secret
    with pytest.raises(TypeError):
        pickle.dumps(secret)
    with pytest.raises(TypeError):
        pickle.dumps(Config(api_key="telmoni_x"))
