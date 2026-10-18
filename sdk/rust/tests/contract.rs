//! The configuration: its defaults, what reads as unset, the key never
//! printed or serialized, and the environment read in one place.

mod common;

use common::{Outcome, in_child, is_child};
use telmoni_sdk::{Config, DEFAULT_ENDPOINT, Telmoni, VERSION};

#[test]
fn the_default_endpoint_is_the_machine_host() {
    assert_eq!(DEFAULT_ENDPOINT, "https://api.telmoni.com");
    assert_eq!(Config::default().endpoint, DEFAULT_ENDPOINT);
    let telmoni = Telmoni::new(Config::default());
    assert_eq!(telmoni.config().endpoint, DEFAULT_ENDPOINT);
    telmoni.shutdown();
}

#[test]
fn a_trailing_slash_is_stripped() {
    assert_eq!(
        Config::new("https://api.telmoni.local/", None, None).endpoint,
        "https://api.telmoni.local"
    );
    assert_eq!(
        Config::new("https://api.telmoni.local///", None, None).endpoint,
        "https://api.telmoni.local"
    );
    // Built by hand, the configuration skips `new`: the client normalizes it.
    let telmoni = Telmoni::new(Config {
        endpoint: "https://custom.endpoint///".to_owned(),
        ..Config::default()
    });
    assert_eq!(telmoni.config().endpoint, "https://custom.endpoint");
    telmoni.shutdown();
}

#[test]
fn a_blank_endpoint_organization_key_environment_or_release_reads_as_unset() {
    let config = Config::new("  ", Some(" ".to_owned()), Some("\t".to_owned()));
    assert_eq!(config.endpoint, DEFAULT_ENDPOINT);
    assert_eq!(config.organization_id, None);
    assert_eq!(config.api_key, None);

    let telmoni = Telmoni::new(Config {
        endpoint: " ".to_owned(),
        organization_id: Some(" org_1 ".to_owned()),
        api_key: Some("".to_owned()),
        environment: Some(" ".to_owned()),
        release: Some(String::new()),
        ..Config::default()
    });
    assert_eq!(telmoni.config().endpoint, DEFAULT_ENDPOINT);
    assert_eq!(telmoni.config().organization_id.as_deref(), Some("org_1"));
    assert_eq!(telmoni.config().api_key, None);
    assert_eq!(telmoni.config().environment, None);
    assert_eq!(telmoni.config().release, None);
    telmoni.shutdown();

    let deployed = Telmoni::new(Config {
        environment: Some(" staging ".to_owned()),
        release: Some("2026.10.18".to_owned()),
        ..Config::default()
    });
    assert_eq!(deployed.config().environment.as_deref(), Some("staging"));
    assert_eq!(deployed.config().release.as_deref(), Some("2026.10.18"));
    deployed.shutdown();
}

#[test]
fn the_key_is_never_printed_or_serialized() -> Outcome {
    let config = Config::new(
        DEFAULT_ENDPOINT,
        Some("org_1".to_owned()),
        Some("telmoni_secret_key".to_owned()),
    );
    let telmoni = Telmoni::new(config.clone());
    for printed in [
        format!("{config:?}"),
        format!("{config:#?}"),
        format!("{telmoni:?}"),
        format!("{telmoni:#?}"),
        serde_json::to_string(&config)?,
    ] {
        assert!(!printed.contains("telmoni_secret_key"), "{printed}");
        assert!(printed.contains("org_1"), "{printed}");
    }
    assert!(format!("{config:?}").contains("<redacted>"));
    assert_eq!(config.api_key.as_deref(), Some("telmoni_secret_key"));
    assert_eq!(
        telmoni.config().api_key.as_deref(),
        Some("telmoni_secret_key")
    );
    telmoni.shutdown();
    Ok(())
}

#[test]
fn the_defaults_send_every_run_and_no_content() {
    let config = Config::default();
    assert!((config.sample_rate - 1.0).abs() < f64::EPSILON);
    assert!(!config.capture_content);
    assert!(config.sampler.is_none());
}

#[test]
fn from_env_reads_the_five_variables_and_no_other() -> Outcome {
    if is_child() {
        let config = Config::from_env();
        assert_eq!(config.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(config.api_key, None);
        assert_eq!(config.organization_id.as_deref(), Some("org_2"));
        assert_eq!(config.environment.as_deref(), Some("staging"));
        assert_eq!(config.release, None);
        return Ok(());
    }
    in_child(
        "from_env_reads_the_five_variables_and_no_other",
        &[
            ("TELMONI_API_KEY", ""),
            ("TELMONI_ORG", "org_2"),
            ("TELMONI_ENVIRONMENT", "staging"),
            ("TELMONI_RELEASE", ""),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "https://elsewhere.example"),
            (
                "OTEL_RESOURCE_ATTRIBUTES",
                "deployment.environment.name=leaked,service.version=leaked",
            ),
        ],
        &["TELMONI_ENDPOINT"],
    )
}

#[test]
fn from_env_normalizes_what_it_reads() -> Outcome {
    if is_child() {
        let config = Config::from_env();
        assert_eq!(config.endpoint, "https://api.telmoni.local");
        assert_eq!(config.api_key.as_deref(), Some("telmoni_k"));
        assert_eq!(config.organization_id.as_deref(), Some("org_3"));
        assert_eq!(config.environment, None);
        assert_eq!(config.release.as_deref(), Some("2026.10.18"));
        return Ok(());
    }
    in_child(
        "from_env_normalizes_what_it_reads",
        &[
            ("TELMONI_ENDPOINT", "https://api.telmoni.local/"),
            ("TELMONI_API_KEY", " telmoni_k "),
            ("TELMONI_ORG", " org_3 "),
            ("TELMONI_RELEASE", " 2026.10.18 "),
        ],
        &["TELMONI_ENVIRONMENT"],
    )
}

#[test]
fn the_version_is_the_crates() {
    assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
}

#[test]
fn the_client_builds_from_env_with_the_real_transport() -> Outcome {
    if is_child() {
        let telmoni = Telmoni::from_env();
        assert_eq!(telmoni.config().endpoint, DEFAULT_ENDPOINT);
        assert_eq!(telmoni.content_mode(), None);
        telmoni.shutdown();
        return Ok(());
    }
    in_child(
        "the_client_builds_from_env_with_the_real_transport",
        &[],
        &["TELMONI_ENDPOINT", "TELMONI_API_KEY", "TELMONI_ORG"],
    )
}

#[test]
fn plain_http_is_refused_except_to_this_machine() -> Outcome {
    for endpoint in [
        "http://localhost:8080",
        "http://api.localhost",
        "http://127.0.0.1:8080/",
        "http://[::1]:8080",
        "https://api.example.com",
    ] {
        let harness =
            common::Harness::with(Config::new(endpoint, None, Some("telmoni_k".to_owned())))?;
        let run = harness
            .telmoni
            .run("bot", telmoni_sdk::RunOptions::default())?;
        run.end();
        harness.flush()?;
        assert!(harness.transport.count() >= 1, "{endpoint} sent nothing");
        let warnings = harness.warnings.all();
        assert!(warnings.is_empty(), "{endpoint}: {warnings:?}");
        harness.telmoni.shutdown();
    }
    for endpoint in ["http://api.example.com", "http://10.0.0.5:8080"] {
        let harness =
            common::Harness::with(Config::new(endpoint, None, Some("telmoni_k".to_owned())))?;
        let run = harness
            .telmoni
            .run("bot", telmoni_sdk::RunOptions::default())?;
        run.end();
        harness.flush()?;
        assert_eq!(
            harness.transport.count(),
            0,
            "{endpoint} sent the key in the clear"
        );
        let warnings = harness.warnings.all();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings
                .first()
                .is_some_and(|warning| warning.contains("unencrypted HTTP")),
            "{warnings:?}"
        );
        harness.telmoni.shutdown();
    }
    Ok(())
}
