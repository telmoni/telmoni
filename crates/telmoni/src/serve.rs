//! `telmoni-server serve`: every module's routes under one listener, and
//! every loop of the process beside it.

use std::sync::Arc;
use std::time::Duration;

use axum::{Router, middleware, routing::get};

use telmoni_shared::config::env_parse;
use telmoni_shared::middleware::service_auth::require_service_secret;

use crate::probes::{health, heartbeat, livez, not_found};
use crate::{App, accept, sweeps};

impl App {
    /// Every module's routes under one router, with the health probes and
    /// the request layers added once.
    pub fn router(self: &Arc<Self>) -> Router {
        let probes = Router::new()
            .route("/health", get(health))
            .route("/livez", get(livez))
            .route("/heartbeat", get(heartbeat))
            .with_state(self.clone());
        let log_level = Router::new()
            .route(
                "/internal/v1/log-level",
                get(telmoni_shared::logging::get_level)
                    .put(telmoni_shared::logging::put_level)
                    .delete(telmoni_shared::logging::reset_level),
            )
            .layer(middleware::from_fn_with_state(
                self.secrets.clone(),
                require_service_secret,
            ));
        // The machine lanes behind the door, each named, then versioned: the
        // read API at `/v1`, the CLI's at `/cli/v1`, and ingest at `/otlp`,
        // whose `/v1/traces` is OTLP's own, nested whole so `/otlp/` is its to
        // answer too. Each lane gates its own, by key, bearer or device code.
        // The probes are the platform's, and carry no version.
        let core = Router::new()
            .merge(probes)
            .merge(log_level)
            .merge(telmoni_auth::router(self.auth.clone()))
            .nest(
                "/v1",
                self.door.v1(telmoni_auth::v1_router(self.auth.clone())),
            )
            .nest(
                "/cli/v1",
                self.door.cli(telmoni_auth::cli_router(self.auth.clone())),
            )
            .nest_service(
                "/otlp",
                self.door
                    .otlp(telmoni_telemetry::otlp_router(self.telemetry.clone())),
            )
            .merge(telmoni_notifications::router(self.notifications.clone()))
            .merge(telmoni_telemetry::router(self.telemetry.clone()))
            .merge(match &self.agent {
                Some(agent) => telmoni_agent::router(agent.clone()),
                None => telmoni_agent::absent_router(self.secrets.clone()),
            });
        self.modules
            .iter()
            .fold(core, |router, module| router.merge(module.router()))
            // A path no lane serves is the server's own problem document, as
            // a lane's is under it.
            .fallback(not_found)
            .layer(middleware::from_fn(
                telmoni_shared::middleware::http::request_scope,
            ))
            .layer(telmoni_shared::middleware::http::trace_layer())
            .layer(telmoni_shared::middleware::http::request_id_layers())
            .layer(middleware::from_fn(
                telmoni_shared::middleware::http::fresh_request_id,
            ))
    }

    /// Run the process until it is told to stop: refuse what a deployed tier
    /// must not run with, start every loop, and serve on `port`.
    /// `RUN_SWEEPS=false` leaves auth's sweeps to `telmoni-server sweep` runs
    /// scheduled elsewhere.
    pub async fn serve(self: Arc<Self>, port: u16) -> anyhow::Result<()> {
        if self.auth.config.allow_test_session && telmoni_shared::envelope::on_deployed_tier() {
            anyhow::bail!(
                "ALLOW_TEST_SESSION is set in a Kubernetes pod; the test door signs in anyone \
                 for whoever holds the service secret, and is for a laptop alone"
            );
        }

        // A wrong width stops the pod; an embeddings endpoint that does not
        // answer only leaves the agent failing until it does, since sign-in
        // and every other module share this process and need none of it.
        if let Some(agent) = &self.agent {
            if let Ok(embedder) = agent.embedder() {
                telmoni_agent::boot::probe_width(embedder).await?;
            }
            agent.spawn();
        }
        tokio::spawn(telmoni_notifications::retention::run(
            self.notifications.clone(),
        ));
        let delivery = tokio::spawn(telmoni_notifications::delivery::run(
            self.notifications.clone(),
        ));
        let purges = tokio::spawn(telmoni_telemetry::sweep::run(self.telemetry.clone()));
        let refusals = tokio::spawn(telmoni_telemetry::ceilings::run(self.telemetry.clone()));
        let alerts = tokio::spawn(telmoni_telemetry::rules::evaluate::run(
            self.telemetry.clone(),
        ));
        // Told as the shutdown comes, so a pass stops between alerts while
        // the server drains.
        tokio::spawn({
            let telemetry = self.telemetry.clone();
            async move {
                telmoni_shared::shutdown::shutdown_signal().await;
                telemetry.alerts_stopping.ask();
            }
        });
        if env_parse("RUN_SWEEPS", true)? {
            sweeps::spawn(self.auth.clone(), &self.sweeps);
        } else {
            tracing::warn!(
                "RUN_SWEEPS is false; the deletion, retention and audit sweeps run only as `telmoni-server sweep`"
            );
        }
        for module in &self.modules {
            module.spawn();
        }

        let addr = format!("0.0.0.0:{port}");
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tracing::info!(
            addr = %addr,
            trusted_proxy_hops = self.door.trusted_proxy_hops(),
            "telmoni listening"
        );

        // With the peer's address on every request, so the door's per-address
        // limits hold a caller that sends no `X-Forwarded-For` too.
        let serve = accept::serve(
            listener,
            self.router(),
            accept::HEAD_WAIT,
            telmoni_shared::shutdown::shutdown_signal(),
        );
        // ⚠ These loops are raced against the server: a dead one would leave
        // a Ready pod that quietly stops delivering, finishing purges or
        // telling owners of their plan's refusals, so the process exits and
        // the restart is the recovery.
        tokio::select! {
            () = serve => {},
            ended = delivery => return died("the delivery loop", ended),
            ended = purges => return died("telemetry's purge sweep", ended),
            ended = refusals => return died("the plan refusals' notices", ended),
            ended = alerts => return died("the alerts' evaluation", ended),
        }
        // The alert in hand is settled and told before the process ends.
        self.telemetry.alerts_stopping.stop(ALERTS_STOP_WAIT).await;
        Ok(())
    }
}

/// A loop raced against the server ended, which it never does on its own.
fn died(name: &str, ended: Result<(), tokio::task::JoinError>) -> anyhow::Result<()> {
    match ended {
        Ok(()) => anyhow::bail!("{name} exited; restarting the process"),
        Err(e) => anyhow::bail!("{name} died: {e}"),
    }
}

/// How long a stopping process waits for an alert pass to finish the alert
/// in hand: well inside the 30 s a pod is given to stop.
const ALERTS_STOP_WAIT: Duration = Duration::from_secs(20);
