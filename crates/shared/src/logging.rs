//! The subscriber every binary installs, and the one lever that changes how
//! much it says.
//!
//! One line per event on stdout, which GKE's logging agent collects: JSON on a
//! pipe, the human formatter on a terminal.
//!
//! ⚠ **The JSON is Cloud Logging's shape, not `tracing`'s.** The agent promotes
//! reserved keys, and `severity` decides whether an entry is an ERROR.
//! `tracing_subscriber`'s `.json()` writes `level`, which is not reserved, so
//! every line — `error!` and `AUDIT CHAIN BREAK` included — landed at INFO and
//! was invisible to a `severity>=ERROR` query. `CloudLoggingLayer` emits the
//! reserved names. `web/lib/logger.ts` does the same for pino.
//!
//! Boot level is `RUST_LOG`, else `LOG_LEVEL`, else `info`, both `.trim()`ed:
//! `dotenvy` returns the padding of an aligned inline comment, and an untrimmed
//! directive does not parse.
//!
//! ⚠ **A raise always expires.** [`set_filter`] swaps the filter live
//! (`PUT /internal/log-level`, `make log-level`) and reverts after a capped
//! TTL, because the failure mode of a debug session is someone forgetting it.
//! A verbosity that should outlive an incident is a reviewed `LOG_LEVEL` change.

use std::io::{IsTerminal, Write};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Map, Value};
use tracing::field::{Field, Visit};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

/// How long a raised level lasts when the caller names no TTL.
pub const DEFAULT_TTL_SECS: u64 = 900;

/// The longest a raised level may last. A debug session that wants more than
/// an hour wants a `LOG_LEVEL` change in the deployment instead.
pub const MAX_TTL_SECS: u64 = 3600;

type Handle = reload::Handle<EnvFilter, Registry>;

static RELOAD: OnceLock<Handle> = OnceLock::new();
/// What the process booted with — the level a raise reverts TO.
static BOOT: OnceLock<String> = OnceLock::new();
/// What is in force now. Read by `GET /internal/log-level`.
static CURRENT: Mutex<String> = Mutex::new(String::new());
/// Bumped on every change, so a pending revert that has been superseded by a
/// newer raise does nothing instead of stamping on it.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Take a lock, recovering from poisoning rather than propagating it.
fn lock<T>(m: &'static Mutex<T>) -> std::sync::MutexGuard<'static, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// ⚠ **A ceiling on the layer that writes, which no directive and no raise
/// can lift.** The `clickhouse` client logs a failed query's error at
/// `debug`, and the error ClickHouse answers can carry a value the query
/// bound, unquoted. A directive pinned in the filter would not hold it: one
/// narrower than the crate — a module, a field, a span — wins over it.
fn ceiling() -> Targets {
    Targets::new()
        .with_default(LevelFilter::TRACE)
        .with_target("clickhouse", LevelFilter::INFO)
}

/// The boot directive: `RUST_LOG`, else `LOG_LEVEL`, else `info`.
fn boot_directive() -> String {
    for name in ["RUST_LOG", "LOG_LEVEL"] {
        if let Ok(raw) = std::env::var(name) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "info".to_string()
}

/// Install the subscriber. Call once, first thing in `main()`.
pub fn init() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let directive = boot_directive();
    let filter = EnvFilter::try_new(&directive)?;
    let (layer, handle) = reload::Layer::new(filter);

    let json = !std::io::stdout().is_terminal();
    let registry = Registry::default().with(layer);
    if json {
        registry
            .with(CloudLoggingLayer.with_filter(ceiling()))
            .try_init()?;
    } else {
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(true)
                    .with_filter(ceiling()),
            )
            .try_init()?;
    }

    RELOAD
        .set(handle)
        .map_err(|_| "logging already initialised")?;
    BOOT.set(directive.clone())
        .map_err(|_| "logging already initialised")?;
    *lock(&CURRENT) = directive;
    Ok(())
}

/// Cloud Logging's severity for a `tracing` level.
fn severity(level: tracing::Level) -> &'static str {
    match level {
        tracing::Level::TRACE | tracing::Level::DEBUG => "DEBUG",
        tracing::Level::INFO => "INFO",
        tracing::Level::WARN => "WARNING",
        tracing::Level::ERROR => "ERROR",
    }
}

/// Collects `tracing` fields into a JSON object, keeping numbers and booleans
/// as themselves, so a log-based metric can read one without parsing text.
struct JsonVisitor<'a>(&'a mut Map<String, Value>);

impl Visit for JsonVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.0
            .insert(field.name().to_owned(), value.to_string().into());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_owned(), format!("{value:?}").into());
    }
}

/// A span's fields, parked in its extensions so every event inside it can be
/// stamped with them.
struct SpanFields(Map<String, Value>);

/// Assemble one Cloud Logging entry.
fn render(
    severity: &str,
    target: &str,
    source: Option<(&str, u32)>,
    event_name: &str,
    mut entry: Map<String, Value>,
) -> Map<String, Value> {
    if !entry.contains_key("message") {
        entry.insert("message".to_owned(), event_name.into());
    }
    entry.insert("severity".to_owned(), severity.into());
    entry.insert("target".to_owned(), target.into());
    entry.insert(
        "timestamp".to_owned(),
        chrono::Utc::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
            .into(),
    );
    if let Some((file, line)) = source {
        entry.insert(
            "logging.googleapis.com/sourceLocation".to_owned(),
            serde_json::json!({ "file": file, "line": line.to_string() }),
        );
    }
    entry
}

/// Writes each event as one Cloud Logging structured line on stdout.
struct CloudLoggingLayer;

impl<S> Layer<S> for CloudLoggingLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        let Some(span) = ctx.span(id) else { return };
        let mut fields = Map::new();
        attrs.record(&mut JsonVisitor(&mut fields));
        span.extensions_mut().insert(SpanFields(fields));
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: Context<'_, S>,
    ) {
        let Some(span) = ctx.span(id) else { return };
        let mut extensions = span.extensions_mut();
        if let Some(SpanFields(fields)) = extensions.get_mut::<SpanFields>() {
            values.record(&mut JsonVisitor(fields));
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut entry = Map::new();

        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                if let Some(SpanFields(fields)) = span.extensions().get::<SpanFields>() {
                    for (name, value) in fields {
                        entry.insert(name.clone(), value.clone());
                    }
                }
            }
        }
        event.record(&mut JsonVisitor(&mut entry));

        let entry = render(
            severity(*metadata.level()),
            metadata.target(),
            metadata.file().zip(metadata.line()),
            metadata.name(),
            entry,
        );

        let Ok(line) = serde_json::to_string(&Value::Object(entry)) else {
            return;
        };
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let _ = writeln!(out, "{line}");
    }
}

/// The directive in force. Empty before [`init`] runs.
#[must_use]
pub fn current() -> String {
    lock(&CURRENT).clone()
}

/// The directive the process booted with — what a raise reverts to.
#[must_use]
pub fn boot() -> String {
    BOOT.get().cloned().unwrap_or_default()
}

/// Seconds until the current raise expires, or `None` when the boot level is
/// in force.
#[must_use]
pub fn expires_in() -> Option<u64> {
    let deadline = *lock(&DEADLINE);
    deadline.map(|d| {
        d.saturating_duration_since(std::time::Instant::now())
            .as_secs()
    })
}

static DEADLINE: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// The TTL a raise actually gets: the caller's, else [`DEFAULT_TTL_SECS`],
/// clamped into `1..=MAX_TTL_SECS`. Zero clamps UP: the safe reading of an
/// incoherent request on a cost lever is the smallest raise, never an unbounded one.
fn resolve_ttl(ttl_secs: Option<u64>) -> u64 {
    ttl_secs.unwrap_or(DEFAULT_TTL_SECS).clamp(1, MAX_TTL_SECS)
}

/// Swap the filter on a running process for a capped `ttl_secs`, then revert
/// to the boot directive.
pub fn set_filter(
    directives: &str,
    ttl_secs: Option<u64>,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let directives = directives.trim();
    let handle = RELOAD.get().ok_or("logging is not initialised")?;
    let filter = EnvFilter::try_new(directives)?;
    let ttl = resolve_ttl(ttl_secs);

    handle.reload(filter)?;
    *lock(&CURRENT) = directives.to_string();
    let deadline = std::time::Instant::now() + Duration::from_secs(ttl);
    *lock(&DEADLINE) = Some(deadline);
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    tracing::warn!(
        filter = %directives,
        ttl_secs = ttl,
        "log level raised; reverting automatically"
    );

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(ttl)).await;
        if GENERATION.load(Ordering::SeqCst) == generation {
            revert();
        }
    });
    Ok(ttl)
}

/// Return to the boot directive now, cancelling any pending revert.
pub fn revert() {
    let Some(handle) = RELOAD.get() else { return };
    let boot = boot();
    let Ok(filter) = EnvFilter::try_new(&boot) else {
        return;
    };
    if handle.reload(filter).is_ok() {
        *lock(&CURRENT) = boot.clone();
        *lock(&DEADLINE) = None;
        GENERATION.fetch_add(1, Ordering::SeqCst);
        tracing::warn!(filter = %boot, "log level reverted to the boot directive");
    }
}

/// `GET /internal/log-level` — what this process is logging, and for how long.
#[expect(
    clippy::unused_async,
    reason = "an axum handler must be async even when its body is not; the lint does not fire on the services' own handlers only because they are passed as values in the same crate"
)]
pub async fn get_level() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "current": current(),
        "boot": boot(),
        "expires_in_secs": expires_in(),
        "default_ttl_secs": DEFAULT_TTL_SECS,
        "max_ttl_secs": MAX_TTL_SECS,
    }))
}

/// The body of `PUT /internal/log-level`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetLevelRequest {
    /// Any `EnvFilter` directive — `debug`, or
    /// `info,telmoni_auth::handler::deletion=debug`.
    pub filter: String,
    /// Seconds before the automatic revert, capped. There is no "until I say
    /// otherwise".
    #[serde(default)]
    pub ttl_secs: Option<u64>,
}

/// `PUT /internal/log-level` — raise (or lower) verbosity on a running pod.
#[expect(
    clippy::unused_async,
    reason = "an axum handler must be async even when its body is not; the lint does not fire on the services' own handlers only because they are passed as values in the same crate"
)]
pub async fn put_level(
    crate::extract::Json(req): crate::extract::Json<SetLevelRequest>,
) -> Result<axum::Json<serde_json::Value>, crate::TelmoniError> {
    let ttl = set_filter(&req.filter, req.ttl_secs)
        .map_err(|e| crate::AuthError::BadRequest(format!("not a log filter: {e}")))?;
    Ok(axum::Json(serde_json::json!({
        "current": current(),
        "boot": boot(),
        "expires_in_secs": ttl,
    })))
}

/// `DELETE /internal/log-level` — drop back to the boot directive now, rather
/// than waiting out the TTL.
#[expect(
    clippy::unused_async,
    reason = "an axum handler must be async even when its body is not; the lint does not fire on the services' own handlers only because they are passed as values in the same crate"
)]
pub async fn reset_level() -> axum::Json<serde_json::Value> {
    revert();
    axum::Json(serde_json::json!({ "current": current(), "boot": boot() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The directive is validated before it is applied.
    #[test]
    fn a_bad_directive_is_rejected() {
        assert!(EnvFilter::try_new("=====").is_err());
        assert!(EnvFilter::try_new("info,telmoni_auth=debug").is_ok());
    }

    /// The TTL is clamped at both ends. This is the cost guarantee: no input
    /// buys unbounded debug logging.
    #[test]
    fn the_ttl_is_clamped_to_the_cap() {
        assert_eq!(resolve_ttl(None), DEFAULT_TTL_SECS);
        assert_eq!(resolve_ttl(Some(0)), 1);
        assert_eq!(resolve_ttl(Some(99_999)), MAX_TTL_SECS);
        assert_eq!(resolve_ttl(Some(60)), 60);
    }

    /// `dotenvy` returns the padding from an aligned inline comment, and an
    /// untrimmed directive does not parse.
    #[test]
    fn a_padded_directive_still_parses() {
        assert!(EnvFilter::try_new("info   ".trim()).is_ok());
    }

    /// ⚠ No directive gets the `clickhouse` client written past `info`, a
    /// raise's included, nor one narrower than the crate, which would win
    /// over a directive pinned beside it: its `debug` carries ClickHouse's
    /// errors, which can carry a query's values.
    #[test]
    fn the_clickhouse_client_is_never_written_below_info() {
        struct Written(std::sync::Arc<AtomicU64>);
        impl<S: tracing::Subscriber> Layer<S> for Written {
            fn on_event(&self, _event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        for directives in [
            "trace",
            "clickhouse=debug",
            "info,clickhouse::error=debug",
            "clickhouse::error=trace",
        ] {
            let written = std::sync::Arc::new(AtomicU64::new(0));
            let subscriber = Registry::default()
                .with(EnvFilter::try_new(directives).unwrap())
                .with(Written(written.clone()).with_filter(ceiling()));
            tracing::subscriber::with_default(subscriber, || {
                tracing::debug!(target: "clickhouse::error", "a failed query's error");
                tracing::info!(target: "clickhouse::error", "kept");
            });
            assert_eq!(
                written.load(Ordering::SeqCst),
                1,
                "{directives}: the client's debug line was written, or its info line was not"
            );
        }
        assert!(ceiling().would_enable("telmoni_auth::handler", &tracing::Level::TRACE));
    }

    /// The five `tracing` levels, in the four spellings GCP reserves.
    #[test]
    fn every_level_maps_to_a_gcp_severity() {
        assert_eq!(severity(tracing::Level::TRACE), "DEBUG");
        assert_eq!(severity(tracing::Level::DEBUG), "DEBUG");
        assert_eq!(severity(tracing::Level::INFO), "INFO");
        assert_eq!(severity(tracing::Level::WARN), "WARNING");
        assert_eq!(severity(tracing::Level::ERROR), "ERROR");
    }

    /// ⚠ The regression this module exists to prevent.
    #[test]
    fn an_error_is_severity_error_and_not_level() {
        let entry = render(
            "ERROR",
            "telmoni_auth",
            None,
            "event src/x.rs:1",
            Map::new(),
        );
        assert_eq!(entry["severity"], "ERROR");
        assert!(!entry.contains_key("level"), "`level` is not reserved");
    }

    /// The event's own `message` is the summary line, and the fields beside it
    /// survive as themselves.
    #[test]
    fn the_message_and_the_fields_both_survive() {
        let mut fields = Map::new();
        fields.insert("message".to_owned(), "audit chain break".into());
        fields.insert("rows".to_owned(), 41.into());
        let entry = render("ERROR", "telmoni_auth", None, "event src/x.rs:1", fields);
        assert_eq!(entry["message"], "audit chain break");
        assert_eq!(entry["rows"], 41);
    }

    /// An event with no message field still shows something in the summary
    /// column rather than a blank row.
    #[test]
    fn a_message_less_event_falls_back_to_its_name() {
        let entry = render("INFO", "telmoni_auth", None, "event src/x.rs:9", Map::new());
        assert_eq!(entry["message"], "event src/x.rs:9");
    }

    /// `sourceLocation.line` is an int64 over JSON, which means a STRING; a
    /// number there is dropped by the agent, taking the file with it.
    #[test]
    fn the_source_location_line_is_a_string() {
        let entry = render(
            "WARNING",
            "telmoni_auth",
            Some(("crates/auth/src/lib.rs", 364)),
            "event src/x.rs:1",
            Map::new(),
        );
        let source = &entry["logging.googleapis.com/sourceLocation"];
        assert_eq!(source["file"], "crates/auth/src/lib.rs");
        assert_eq!(source["line"], "364");
    }
}
