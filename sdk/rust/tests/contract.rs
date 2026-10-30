//! The configuration: its defaults, what reads as unset, the key never
//! printed or serialized, and the environment read in one place.

mod common;

use common::{Harness, Outcome, in_child, is_child, keyed};
use telmoni::{Config, DEFAULT_ENDPOINT, RunOptions, Telmoni, VERSION};

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
    in_child::<&str>(
        "the_client_builds_from_env_with_the_real_transport",
        &[],
        &["TELMONI_ENDPOINT", "TELMONI_API_KEY", "TELMONI_ORG"],
    )
}

/// Read as unset, it sent to the hosted default with the key: refused instead.
#[cfg(unix)]
#[test]
fn an_endpoint_variable_that_is_no_unicode_is_refused() -> Outcome {
    use std::os::unix::ffi::OsStrExt as _;

    if is_child() {
        let harness = common::Harness::with(Config::from_env())?;
        harness
            .telmoni
            .run("bot", telmoni::RunOptions::default())?
            .end();
        harness.flush()?;
        harness.telmoni.shutdown();
        assert_eq!(harness.transport.count(), 0, "it was sent somewhere");
        let warnings = harness.warnings.all();
        assert!(
            warnings
                .first()
                .is_some_and(|warning| warning.contains("http:// or https://")),
            "{warnings:?}"
        );
        return Ok(());
    }
    in_child(
        "an_endpoint_variable_that_is_no_unicode_is_refused",
        &[(
            "TELMONI_ENDPOINT",
            std::ffi::OsStr::from_bytes(b"http://localhost:4318\xc3"),
        )],
        &[],
    )
}

/// As `sdk/fixtures/endpoints.json` reads them: plain HTTP to this machine
/// alone, and a host without a scheme as the CLI reads it.
#[test]
fn every_endpoint_is_read_as_the_fixture_reads_it() -> Outcome {
    let fixture = common::load_fixture("endpoints.json")?;
    let taken = common::field(&fixture, "taken")?
        .as_array()
        .ok_or("taken")?;
    for case in taken {
        let given = common::text(case, "given")?;
        let endpoint = common::text(case, "endpoint")?;
        assert_eq!(Config::new(given, None, None).endpoint, endpoint, "{given}");
        assert_eq!(
            Config::new(endpoint, None, None).endpoint,
            endpoint,
            "{given}, read twice"
        );
        let (sent, warnings, _) = one_run(given)?;
        assert!(
            sent >= 1 && warnings.is_empty(),
            "{given}: {sent} sent, {warnings:?}"
        );
    }
    let refused = common::field(&fixture, "refused")?
        .as_array()
        .ok_or("refused")?;
    for case in refused {
        let given = common::text(case, "given")?;
        let why = match common::text(case, "as")? {
            "cleartext" => "unencrypted HTTP",
            _ => "http:// or https://",
        };
        let (sent, warnings, kinds) = one_run(given)?;
        assert_eq!(sent, 0, "{given} was sent to");
        assert!(
            warnings.len() == 1 && warnings.first().is_some_and(|text| text.contains(why)),
            "{given}: {warnings:?}"
        );
        assert_eq!(kinds, ["refused"], "{given}");
    }
    Ok(())
}

/// One run on a client whose endpoint is `given`: the exports it sent, and its
/// warnings and their kinds.
fn one_run(given: &str) -> Result<(usize, Vec<String>, Vec<&'static str>), common::Failure> {
    let harness = common::Harness::with(Config::new(given, None, Some("telmoni_k".to_owned())))?;
    harness
        .telmoni
        .run("bot", telmoni::RunOptions::default())?
        .end();
    harness.flush()?;
    harness.telmoni.shutdown();
    Ok((
        harness.transport.count(),
        harness.warnings.all(),
        harness.warnings.kinds(),
    ))
}

/// A deployment named past ingest's cap, which ingest would read as none
/// without a word, is left off with one warning.
#[test]
fn a_deployment_named_past_ingests_cap_is_left_off_with_one_warning() -> Outcome {
    let harness = Harness::with(Config {
        environment: Some("e".repeat(129)),
        release: Some("r".repeat(128)),
        ..keyed()
    })?;
    harness.telmoni.run("bot", RunOptions::default())?.end();
    harness.flush()?;
    let bodies = harness.transport.bodies()?;
    let body = bodies.first().ok_or("a body")?;
    let keys: Vec<&str> = body["resourceSpans"][0]["resource"]["attributes"]
        .as_array()
        .ok_or("the resource's attributes")?
        .iter()
        .filter_map(|attribute| attribute["key"].as_str())
        .collect();
    assert!(!keys.contains(&"deployment.environment.name"), "{keys:?}");
    assert!(keys.contains(&"service.version"), "{keys:?}");
    assert_eq!(
        harness.warnings.all(),
        ["telmoni: the environment must be up to 128 characters; it is not sent"]
    );
    Ok(())
}
