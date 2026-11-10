//! The configuration: its defaults, what reads as unset, the key never
//! printed or serialized, and the environment read in one place.

mod common;

use common::{
    Failure, Harness, Outcome, attributes_of, field, in_child, is_child, keyed, load_fixture,
    mode_on, only, text,
};
use telmoni::{Config, DEFAULT_ENDPOINT, ModelCallOptions, Telmoni, TraceOptions, VERSION};

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
fn the_defaults_send_no_content_and_set_no_sampler() {
    let config = Config::default();
    assert!(!config.capture_content);
    assert!(config.sampler.is_none());
}

#[test]
fn from_env_reads_the_eight_variables_and_no_other() -> Outcome {
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
        "from_env_reads_the_eight_variables_and_no_other",
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

/// Each case in a child of its own, the value it must read as in a variable beside it.
#[test]
fn capture_content_is_read_from_the_environment_as_the_fixture_says() -> Outcome {
    const WANT: &str = "TELMONI_TEST_CAPTURE_CONTENT";
    if is_child() {
        let want = std::env::var(WANT).as_deref() == Ok("true");
        assert_eq!(Config::from_env().capture_content, want);
        return Ok(());
    }
    let fixture = load_fixture("capture_content.json")?;
    for case in field(&fixture, "cases")?.as_array().ok_or("cases")? {
        let want = if field(case, "capture_content")?.as_bool().ok_or("a case")? {
            "true"
        } else {
            "false"
        };
        let name = "capture_content_is_read_from_the_environment_as_the_fixture_says";
        match field(case, "given")?.as_str() {
            Some(given) => in_child(
                name,
                &[("TELMONI_CAPTURE_CONTENT", given), (WANT, want)],
                &[],
            )?,
            None => in_child(name, &[(WANT, want)], &["TELMONI_CAPTURE_CONTENT"])?,
        }
    }
    Ok(())
}

/// Code sets the opposite of what the environment says, and stands.
#[test]
fn capture_content_set_in_code_wins_over_the_environment() -> Outcome {
    if is_child() {
        let on = std::env::var_os("TELMONI_CAPTURE_CONTENT").is_some();
        let telmoni = Telmoni::new(Config {
            capture_content: !on,
            ..Config::from_env()
        });
        assert_eq!(telmoni.config().capture_content, !on);
        telmoni.shutdown();
        return Ok(());
    }
    let name = "capture_content_set_in_code_wins_over_the_environment";
    in_child(
        name,
        &[("TELMONI_CAPTURE_CONTENT", "1")],
        &["TELMONI_ENDPOINT"],
    )?;
    in_child::<&str>(name, &[], &["TELMONI_CAPTURE_CONTENT", "TELMONI_ENDPOINT"])
}

/// `TELMONI_SCRUB_SECRETS`'s `0` alone turns secrets off, and `TELMONI_SCRUB_PII`'s `1` alone PII on.
#[test]
fn the_scrubbers_are_read_from_the_environment() -> Outcome {
    const WANT: &str = "TELMONI_TEST_SCRUBBERS";
    if is_child() {
        let config = Config::from_env();
        let read = format!("{:?} {:?}", config.scrub_secrets, config.scrub_pii);
        assert_eq!(read, std::env::var(WANT)?);
        return Ok(());
    }
    assert_eq!(
        (Config::default().scrub_secrets, keyed().scrub_pii),
        (None, None)
    );
    for (secrets, pii, want) in [
        (None, None, "Some(true) Some(false)"),
        (Some(" 0\t"), Some(" 1 "), "Some(false) Some(true)"),
        (Some("false"), Some("true"), "Some(true) Some(false)"),
        (Some("off"), Some("yes"), "Some(true) Some(false)"),
        (Some(""), Some(""), "Some(true) Some(false)"),
    ] {
        let mut set = vec![(WANT, want)];
        let mut unset = Vec::new();
        for (variable, given) in [
            ("TELMONI_SCRUB_SECRETS", secrets),
            ("TELMONI_SCRUB_PII", pii),
        ] {
            match given {
                Some(given) => set.push((variable, given)),
                None => unset.push(variable),
            }
        }
        in_child("the_scrubbers_are_read_from_the_environment", &set, &unset)?;
    }
    Ok(())
}

/// Unset in code, the variables decide what content leaves scrubbed; set in code, the code does.
#[test]
fn the_scrubbers_set_in_code_win_over_the_environment() -> Outcome {
    if is_child() {
        let planted = field(&load_fixture("canary.json")?, "planted")?.clone();
        let (key, email) = (text(&planted, "cloud_key")?, text(&planted, "email")?);
        let message = format!("key {key}, mail {email}");
        let sent = |config: Config| -> Result<String, Failure> {
            let harness = Harness::answering(
                Config {
                    capture_content: true,
                    ..config
                },
                std::iter::repeat_n(Ok(mode_on()), 2),
            )?;
            let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
            let call = trace.model_call(
                "m",
                ModelCallOptions {
                    input_messages: Some(message.as_str().into()),
                    ..ModelCallOptions::default()
                },
            )?;
            call.end();
            trace.end();
            harness.flush()?;
            let spans = harness.spans()?;
            Ok(text(
                &attributes_of(only(&spans, "chat ")?)?,
                "gen_ai.input.messages",
            )?
            .to_owned())
        };
        let scrubbed = |secrets: bool, pii: bool| {
            let key = if secrets { "[secret: aws_key]" } else { key };
            let email = if pii { "[pii: email]" } else { email };
            format!("key {key}, mail {email}")
        };
        // The first child's variables turn secrets off and PII on; the second's are unset.
        let flipped = std::env::var_os("TELMONI_SCRUB_SECRETS").is_some();
        assert_eq!(sent(keyed())?, scrubbed(!flipped, flipped));
        let set = Config {
            scrub_secrets: Some(flipped),
            scrub_pii: Some(!flipped),
            ..keyed()
        };
        assert_eq!(sent(set)?, scrubbed(flipped, !flipped));
        return Ok(());
    }
    let name = "the_scrubbers_set_in_code_win_over_the_environment";
    in_child(
        name,
        &[("TELMONI_SCRUB_SECRETS", "0"), ("TELMONI_SCRUB_PII", "1")],
        &[],
    )?;
    in_child::<&str>(name, &[], &["TELMONI_SCRUB_SECRETS", "TELMONI_SCRUB_PII"])
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
            .trace("bot", telmoni::TraceOptions::default())?
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
        let (sent, warnings, _) = one_trace(given)?;
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
        let (sent, warnings, kinds) = one_trace(given)?;
        assert_eq!(sent, 0, "{given} was sent to");
        assert!(
            warnings.len() == 1 && warnings.first().is_some_and(|text| text.contains(why)),
            "{given}: {warnings:?}"
        );
        assert_eq!(kinds, ["refused"], "{given}");
    }
    Ok(())
}

/// One trace on a client whose endpoint is `given`: the exports it sent, and
/// its warnings and their kinds.
fn one_trace(given: &str) -> Result<(usize, Vec<String>, Vec<&'static str>), common::Failure> {
    let harness = common::Harness::with(Config::new(given, None, Some("telmoni_k".to_owned())))?;
    harness
        .telmoni
        .trace("bot", telmoni::TraceOptions::default())?
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
    harness.telmoni.trace("bot", TraceOptions::default())?.end();
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
