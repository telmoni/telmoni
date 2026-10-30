//! The probes, and the server's answer for a path no lane serves.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;

use crate::App;

/// `GET /health` — **readiness**: every module reaches its own tables, or
/// the process is not ready. The chart's readiness probe; `/livez` is its liveness.
pub(crate) async fn health(State(app): State<Arc<App>>) -> (StatusCode, Json<serde_json::Value>) {
    let mut degraded = false;
    let mut report = |module: &str, result: sqlx::Result<()>| {
        if let Err(e) = result {
            degraded = true;
            tracing::error!(module, error = %e, "readiness probe failed");
        }
    };
    report("auth", app.auth.ready().await);
    report("notifications", app.notifications.ready().await);
    report("telemetry", app.telemetry.ready().await);
    if let Some(agent) = &app.agent {
        report("agent", agent.ready().await);
    }
    for module in &app.modules {
        report(module.name(), module.ready().await);
    }
    if degraded {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "degraded" })),
        )
    } else {
        (StatusCode::OK, Json(json!({ "status": "ok" })))
    }
}

/// How long a replica's answer to the watchdog stands: the stores are read at
/// most this often, however often `/heartbeat` is asked, so an uptime check
/// asking from several regions each minute costs one read.
const HEARTBEAT_HELD_FOR: Duration = Duration::from_secs(60);

/// `GET /heartbeat` — the **external watchdog's**, on the machine host: `200`,
/// or `503` naming each loop of the process that no replica has beaten for
/// three of its periods, and nothing else. Never a probe of the chart's: a
/// stopped loop is a page, not a restart.
pub(crate) async fn heartbeat(
    State(app): State<Arc<App>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let stale = app.stale().await;
    if stale.is_empty() {
        (StatusCode::OK, Json(json!({ "status": "ok" })))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "stale", "stale": stale })),
        )
    }
}

impl App {
    /// Every loop of the process that no replica has beaten for three of its
    /// periods, read once [`HEARTBEAT_HELD_FOR`] has passed and by one caller
    /// at a time, so a flood of `/heartbeat` holds no pool.
    async fn stale(&self) -> Vec<String> {
        let mut held = self.watchdog.lock().await;
        if let Some((at, stale)) = held.as_ref()
            && at.elapsed() < HEARTBEAT_HELD_FOR
        {
            return stale.clone();
        }
        let mut stale = self.sweeps.stale(&self.auth.db).await;
        stale.extend(self.notifications.stale().await);
        stale.extend(self.telemetry.stale().await);
        if let Some(agent) = &self.agent {
            stale.extend(agent.stale().await);
        }
        for module in &self.modules {
            stale.extend(module.stale().await);
        }
        *held = Some((Instant::now(), stale.clone()));
        stale
    }
}

/// `GET /livez` — **liveness**. Process-up only, no I/O: a liveness probe that
/// failed on a DB stall would restart every replica at once, and a DB hiccup
/// must never restart the delivery loop.
pub(crate) async fn livez() -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

/// A path no lane serves: `404` as a problem document, as each lane answers
/// one for a path under it, so a client reads one shape from the server
/// wherever it knocks.
pub(crate) async fn not_found() -> axum::response::Response {
    telmoni_shared::middleware::problem::not_found("no such lane")
}
