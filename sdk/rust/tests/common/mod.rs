//! What every suite here shares: a transport that records every export and
//! answers from a script, then with `kept.json`; collectors for the warnings
//! and the sleeps; the fixtures; what a span's JSON holds; and a way to run
//! one test in a child process with an environment of its own, since setting
//! a variable in this process is `unsafe` and would reach the other tests.

use std::collections::{BTreeMap, VecDeque};
use std::error::Error;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use telmoni::transport::{Answer, Transport, TransportError};
use telmoni::{Config, DEFAULT_ENDPOINT, Hooks, Telmoni, Warning};

pub(crate) type Failure = Box<dyn Error>;
pub(crate) type Outcome = Result<(), Failure>;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One request the transport was given.
#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

impl Request {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Records every export and answers from a script, then with `kept.json`.
/// Cloning shares it, so a test keeps a handle while the client owns one.
#[derive(Clone)]
pub(crate) struct FakeTransport {
    inner: Arc<Inner>,
}

struct Inner {
    requests: Mutex<Vec<Request>>,
    answers: Mutex<VecDeque<Result<Answer, TransportError>>>,
    received: Condvar,
    kept: Answer,
    /// While closed, `send` holds the caller after recording its request, so
    /// a test can act while the worker is inside an export.
    gate: Mutex<bool>,
    opened: Condvar,
}

impl FakeTransport {
    pub(crate) fn new(
        answers: impl IntoIterator<Item = Result<Answer, TransportError>>,
    ) -> Result<Self, Failure> {
        Ok(Self {
            inner: Arc::new(Inner {
                requests: Mutex::new(Vec::new()),
                answers: Mutex::new(answers.into_iter().collect()),
                received: Condvar::new(),
                kept: kept()?,
                gate: Mutex::new(false),
                opened: Condvar::new(),
            }),
        })
    }

    pub(crate) fn requests(&self) -> Vec<Request> {
        lock(&self.inner.requests).clone()
    }

    pub(crate) fn count(&self) -> usize {
        lock(&self.inner.requests).len()
    }

    /// Whether `count` requests arrived within five seconds.
    pub(crate) fn wait_for(&self, count: usize) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut requests = lock(&self.inner.requests);
        while requests.len() < count {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            requests = self
                .inner
                .received
                .wait_timeout(requests, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Every body received, decoded.
    pub(crate) fn bodies(&self) -> Result<Vec<Value>, Failure> {
        self.requests()
            .iter()
            .map(|request| Ok(serde_json::from_slice(&request.body)?))
            .collect()
    }

    /// Every span received, in the order it was sent.
    pub(crate) fn spans(&self) -> Result<Vec<Value>, Failure> {
        Ok(self.bodies()?.iter().flat_map(spans_of).collect())
    }

    /// Everything received, as text: what a "never sent" check looks through.
    pub(crate) fn text(&self) -> String {
        self.requests()
            .iter()
            .map(|request| {
                format!(
                    "{} {:?} {}",
                    request.url,
                    request.headers,
                    String::from_utf8_lossy(&request.body)
                )
            })
            .collect()
    }

    pub(crate) fn close_gate(&self) {
        *lock(&self.inner.gate) = true;
    }

    pub(crate) fn open_gate(&self) {
        *lock(&self.inner.gate) = false;
        self.inner.opened.notify_all();
    }
}

impl Transport for FakeTransport {
    fn send(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<Answer, TransportError> {
        let answer = {
            let mut requests = lock(&self.inner.requests);
            requests.push(Request {
                url: url.to_owned(),
                headers: headers.to_vec(),
                body: body.to_vec(),
            });
            let answer = lock(&self.inner.answers)
                .pop_front()
                .unwrap_or_else(|| Ok(self.inner.kept.clone()));
            self.inner.received.notify_all();
            answer
        };
        let mut closed = lock(&self.inner.gate);
        while *closed {
            closed = self
                .inner
                .opened
                .wait(closed)
                .unwrap_or_else(PoisonError::into_inner);
        }
        answer
    }
}

/// What a hook was given, in order.
#[derive(Clone)]
pub(crate) struct Collected<T>(Arc<Mutex<Vec<T>>>);

impl<T> Default for Collected<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
}

impl<T: Clone> Collected<T> {
    pub(crate) fn push(&self, item: T) {
        lock(&self.0).push(item);
    }

    pub(crate) fn all(&self) -> Vec<T> {
        lock(&self.0).clone()
    }
}

/// Each warning's line and what it describes, as the sink was given them.
#[derive(Clone, Default)]
pub(crate) struct Warnings(Collected<(String, Warning)>);

impl Warnings {
    pub(crate) fn push(&self, text: &str, warning: &Warning) {
        self.0.push((text.to_owned(), warning.clone()));
    }

    /// The lines, in order.
    pub(crate) fn all(&self) -> Vec<String> {
        self.0.all().into_iter().map(|(text, _)| text).collect()
    }

    /// What each described, in order, as an answer's `fields` spells it.
    pub(crate) fn fields(&self) -> Vec<Value> {
        self.0
            .all()
            .into_iter()
            .map(|(_, warning)| {
                let mut fields = Map::new();
                fields.insert("kind".to_owned(), json!(warning.kind.as_str()));
                if let Some(status) = warning.status {
                    fields.insert("status".to_owned(), json!(status));
                }
                if let Some(problem_type) = warning.problem_type {
                    fields.insert("problem_type".to_owned(), json!(problem_type));
                }
                Value::Object(fields)
            })
            .collect()
    }

    /// Each warning's kind, in order.
    pub(crate) fn kinds(&self) -> Vec<&'static str> {
        self.0
            .all()
            .into_iter()
            .map(|(_, warning)| warning.kind.as_str())
            .collect()
    }
}

pub(crate) type Sleeps = Collected<Duration>;

/// The client's hooks on these fakes, the rest as the client has them.
pub(crate) fn hooks(transport: &FakeTransport, warnings: &Warnings, sleeps: &Sleeps) -> Hooks {
    let warned = warnings.clone();
    let slept = sleeps.clone();
    Hooks {
        transport: Box::new(transport.clone()),
        sleep: Box::new(move |duration| slept.push(duration)),
        warn: Box::new(move |text, warning| warned.push(text, warning)),
        ..Hooks::default()
    }
}

pub(crate) fn client(
    config: Config,
    transport: &FakeTransport,
    warnings: &Warnings,
    sleeps: &Sleeps,
) -> Telmoni {
    Telmoni::with_hooks(config, hooks(transport, warnings, sleeps))
}

/// The default configuration with a key, as the suites run.
pub(crate) fn keyed() -> Config {
    Config::new(DEFAULT_ENDPOINT, None, Some("telmoni_test_key".to_owned()))
}

/// A client on the fakes, with the fakes beside it.
pub(crate) struct Harness {
    pub(crate) transport: FakeTransport,
    pub(crate) warnings: Warnings,
    pub(crate) sleeps: Sleeps,
    pub(crate) telmoni: Telmoni,
}

impl Harness {
    pub(crate) fn new() -> Result<Self, Failure> {
        Self::with(keyed())
    }

    pub(crate) fn with(config: Config) -> Result<Self, Failure> {
        Self::answering(config, [])
    }

    pub(crate) fn answering(
        config: Config,
        answers: impl IntoIterator<Item = Result<Answer, TransportError>>,
    ) -> Result<Self, Failure> {
        let transport = FakeTransport::new(answers)?;
        let warnings = Warnings::default();
        let sleeps = Sleeps::default();
        let telmoni = client(config, &transport, &warnings, &sleeps);
        Ok(Self {
            transport,
            warnings,
            sleeps,
            telmoni,
        })
    }

    pub(crate) fn spans(&self) -> Result<Vec<Value>, Failure> {
        self.transport.spans()
    }

    pub(crate) fn flush(&self) -> Outcome {
        if self.telmoni.flush() {
            Ok(())
        } else {
            Err("the flush timed out".into())
        }
    }
}

// ── Fixtures ─────────────────────────────────────────────────────────────────

pub(crate) fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
}

pub(crate) fn load_fixture(name: &str) -> Result<Value, Failure> {
    let text = std::fs::read_to_string(fixtures_dir().join(name))?;
    Ok(serde_json::from_str(&text)?)
}

/// An answer fixture's `status`, `headers` and `body`.
pub(crate) fn answer_from(fixture: &Value) -> Result<Answer, Failure> {
    let status = u16::try_from(field(fixture, "status")?.as_u64().ok_or("a status")?)?;
    let headers = field(fixture, "headers")?
        .as_object()
        .ok_or("headers")?
        .iter()
        .map(|(name, value)| {
            Ok((
                name.clone(),
                value.as_str().ok_or("a header value")?.to_owned(),
            ))
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    let body = serde_json::to_vec(field(fixture, "body")?)?;
    Ok(Answer::new(status, headers, body))
}

/// `answers/<name>.json`, as an answer.
pub(crate) fn answer(name: &str) -> Result<Answer, Failure> {
    answer_from(&load_fixture(&format!("answers/{name}.json"))?)
}

pub(crate) fn kept() -> Result<Answer, Failure> {
    answer("kept")
}

/// Every answer under `answers/`, by name.
pub(crate) fn answer_names() -> Result<Vec<String>, Failure> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(fixtures_dir().join("answers"))? {
        let file = entry?.file_name();
        if let Some(name) = file.to_str().and_then(|file| file.strip_suffix(".json")) {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// The line `answers/<name>.json` says its answer logs, met on every attempt: none, or that one.
pub(crate) fn warning_of(name: &str) -> Result<Vec<String>, Failure> {
    let fixture = load_fixture(&format!("answers/{name}.json"))?;
    match field(&fixture, "warning")? {
        Value::Null => Ok(Vec::new()),
        warning => Ok(vec![
            warning
                .as_str()
                .ok_or("`warning` is not a string")?
                .to_owned(),
        ]),
    }
}

/// An answer's `fields`: what its one warning describes, or none.
pub(crate) fn fields_of(name: &str) -> Result<Vec<Value>, Failure> {
    let fixture = load_fixture(&format!("answers/{name}.json"))?;
    match field(&fixture, "fields")? {
        Value::Null => Ok(Vec::new()),
        fields => Ok(vec![fields.clone()]),
    }
}

// ── JSON ─────────────────────────────────────────────────────────────────────

pub(crate) fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, Failure> {
    value
        .get(key)
        .ok_or_else(|| format!("no `{key}` in {value}").into())
}

pub(crate) fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, Failure> {
    field(value, key)?
        .as_str()
        .ok_or_else(|| format!("`{key}` is not a string").into())
}

pub(crate) fn int(value: &Value, key: &str) -> Result<i64, Failure> {
    field(value, key)?
        .as_i64()
        .ok_or_else(|| format!("`{key}` is not an integer").into())
}

pub(crate) fn float(value: &Value, key: &str) -> Result<f64, Failure> {
    field(value, key)?
        .as_f64()
        .ok_or_else(|| format!("`{key}` is not a number").into())
}

/// A body's spans, across its resource and scope groups.
pub(crate) fn spans_of(body: &Value) -> Vec<Value> {
    body.get("resourceSpans")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|resource_spans| resource_spans.get("scopeSpans")?.as_array())
        .flatten()
        .filter_map(|scope_spans| scope_spans.get("spans")?.as_array())
        .flatten()
        .cloned()
        .collect()
}

pub(crate) fn name_of(span: &Value) -> &str {
    span.get("name").and_then(Value::as_str).unwrap_or("")
}

/// A span's attributes as one object: an `intValue` as a number, the rest as
/// the one typed value each holds.
pub(crate) fn attributes_of(span: &Value) -> Result<Value, Failure> {
    let mut out = Map::new();
    for attribute in field(span, "attributes")?.as_array().ok_or("attributes")? {
        let key = text(attribute, "key")?;
        let value = field(attribute, "value")?
            .as_object()
            .ok_or("a typed value")?;
        let (kind, inner) = value.iter().next().ok_or("an empty value")?;
        let decoded = if kind == "intValue" {
            json!(inner.as_str().ok_or("an intValue string")?.parse::<i64>()?)
        } else {
            inner.clone()
        };
        out.insert(key.to_owned(), decoded);
    }
    Ok(Value::Object(out))
}

pub(crate) fn named<'a>(spans: &'a [Value], name: &str) -> Result<&'a Value, Failure> {
    spans
        .iter()
        .find(|span| name_of(span) == name)
        .ok_or_else(|| format!("no span named {name}").into())
}

pub(crate) fn starting_with<'a>(spans: &'a [Value], prefix: &str) -> Vec<&'a Value> {
    spans
        .iter()
        .filter(|span| name_of(span).starts_with(prefix))
        .collect()
}

/// The one span whose name starts with `prefix`.
pub(crate) fn only<'a>(spans: &'a [Value], prefix: &str) -> Result<&'a Value, Failure> {
    let found = starting_with(spans, prefix);
    match found.as_slice() {
        [span] => Ok(span),
        _ => Err(format!("{} spans start with {prefix}", found.len()).into()),
    }
}

/// The run span (`invoke_agent …`) carrying `run_id`.
pub(crate) fn run_span<'a>(spans: &'a [Value], run_id: &str) -> Result<&'a Value, Failure> {
    starting_with(spans, "invoke_agent ")
        .into_iter()
        .find(|span| {
            attributes_of(span)
                .ok()
                .and_then(|attributes| attributes.get("telmoni.run_id").cloned())
                == Some(json!(run_id))
        })
        .ok_or_else(|| format!("no run span for {run_id}").into())
}

/// Lowercase 8-4-4-4-12 hex, version 4.
pub(crate) fn is_uuid_v4(id: &str) -> bool {
    let parts: Vec<&str> = id.split('-').collect();
    let lengths: Vec<usize> = parts.iter().map(|part| part.len()).collect();
    lengths == [8, 4, 4, 4, 12]
        && parts.iter().all(|part| is_hex(part, part.len()))
        && parts.get(2).is_some_and(|part| part.starts_with('4'))
}

pub(crate) fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

pub(crate) fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

// ── A child process ──────────────────────────────────────────────────────────

const CHILD: &str = "TELMONI_SDK_TEST_CHILD";

/// Whether this process is the child one test spawned to run under an
/// environment of its own.
pub(crate) fn is_child() -> bool {
    std::env::var_os(CHILD).is_some()
}

/// Runs the test named `test` in this binary again as a child process, with
/// `set` in its environment and `unset` out of it, and fails as it failed.
pub(crate) fn in_child<V: AsRef<std::ffi::OsStr>>(
    test: &str,
    set: &[(&str, V)],
    unset: &[&str],
) -> Outcome {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env(CHILD, "1");
    for name in unset {
        command.env_remove(name);
    }
    for (name, value) in set {
        command.env(name, value);
    }
    let output = command.output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "the child failed ({}):\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }
}

/// The fakes themselves: a scripted answer comes back in order, then `kept`;
/// a request is recorded and waited for; the fixtures load and a span's JSON
/// reads. Every binary compiles this module, and this test is also what keeps
/// a helper some binary leaves unused from being reported dead.
#[test]
fn the_fakes_work() -> Outcome {
    if is_child() {
        return Ok(());
    }
    let transport = FakeTransport::new([
        Ok(answer("forbidden")?),
        Err(TransportError::new("ConnectionRefused")),
    ])?;
    let first = transport.send(
        "https://api.telmoni.local/otlp/v1/traces",
        &[("Authorization".to_owned(), "Bearer k".to_owned())],
        b"{}",
    )?;
    assert_eq!(first.status, 403);
    assert!(transport.send("u", &[], b"{}").is_err());
    let third = transport.send("u", &[], b"{}")?;
    assert_eq!(third.status, 200);
    assert_eq!(third.header("telmoni-content-mode"), Some("off"));
    assert!(transport.wait_for(3));
    assert_eq!(transport.count(), 3);
    let requests = transport.requests();
    assert_eq!(
        requests
            .first()
            .and_then(|request| request.header("authorization")),
        Some("Bearer k")
    );
    assert!(transport.text().contains("otlp/v1/traces"));
    assert_eq!(transport.bodies()?.len(), 3);
    assert!(transport.spans()?.is_empty());
    transport.close_gate();
    transport.open_gate();

    let fixture = load_fixture("run.json")?;
    assert!(text(field(&fixture, "scenario")?, "capture_content").is_err());
    let run = field(field(&fixture, "scenario")?, "run")?;
    assert!((float(run, "sample_rate")? - 1.0).abs() < f64::EPSILON);
    assert_eq!(
        int(&load_fixture("answers/forbidden.json")?, "status")?,
        403
    );
    assert!(answer_names()?.iter().any(|name| name == "kept"));
    assert_eq!(warning_of("kept")?, Vec::<String>::new());
    assert_eq!(warning_of("forbidden")?.len(), 1);
    assert_eq!(fields_of("kept")?, Vec::<Value>::new());
    assert_eq!(
        fields_of("forbidden")?,
        [json!({ "kind": "refused", "status": 403, "problem_type": "/errors/authz/forbidden" })]
    );

    let span = json!({
        "name": "s",
        "attributes": [
            { "key": "n", "value": { "intValue": "5" } },
            { "key": "t", "value": { "stringValue": "x" } },
            { "key": "telmoni.run_id", "value": { "stringValue": "r" } },
        ],
    });
    assert_eq!(
        attributes_of(&span)?,
        json!({ "n": 5, "t": "x", "telmoni.run_id": "r" })
    );
    assert_eq!(name_of(&span), "s");
    let spans = vec![
        span,
        json!({ "name": "invoke_agent bot", "attributes": [
            { "key": "telmoni.run_id", "value": { "stringValue": "r" } },
        ] }),
    ];
    assert_eq!(name_of(named(&spans, "s")?), "s");
    assert_eq!(starting_with(&spans, "invoke_agent ").len(), 1);
    assert_eq!(name_of(only(&spans, "s")?), "s");
    assert!(only(&spans, "").is_err());
    assert_eq!(name_of(run_span(&spans, "r")?), "invoke_agent bot");
    assert!(is_uuid_v4("0f8fad5b-d9cb-469f-a165-70867728950e"));
    assert!(!is_uuid_v4("0f8fad5b-d9cb-169f-a165-70867728950e"));
    assert_eq!(tags(&[("a", "b")]).get("a").map(String::as_str), Some("b"));

    let harness = Harness::new()?;
    harness.flush()?;
    harness.telmoni.shutdown();
    assert!(harness.warnings.all().is_empty());
    assert!(harness.warnings.fields().is_empty());
    assert!(harness.warnings.kinds().is_empty());
    assert!(harness.sleeps.all().is_empty());
    assert!(harness.spans()?.is_empty());

    in_child::<&str>("common::the_fakes_work", &[], &[])
}
