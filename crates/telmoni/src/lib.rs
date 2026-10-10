//! The Telmoni server: one process, and the modules it is made of.
//!
//! Auth, notifications, telemetry and the agent are libraries. This crate
//! links them, hands each the others through the seams in
//! [`telmoni_shared::seam`], mounts their routers under one listener, and
//! runs their background loops — the delivery loop, the retention sweeps,
//! the agent's indexer, the deletion sweep and the nightly audit walk — as
//! tasks of the same process. Each module still connects as its own database
//! role, so row-level security and the grants stand exactly as they did
//! across services.
//!
//! The binary is the command line over this: `telmoni serve`, and the
//! migrator and each sweep as a subcommand run as a Job. A binary built for
//! another deployment calls [`App::assemble`] with its own identity
//! provider and transport, [`App::mount`]s the modules of its own, and hands
//! the same [`run`] the command it parsed.

#![deny(missing_docs)]

pub mod cli;
pub mod sweeps;

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use axum::http::{HeaderMap, StatusCode};
use axum::{Json, Router, middleware, routing::get};
use serde_json::json;
use sqlx::PgPool;

use telmoni_shared::acting::Acting;
use telmoni_shared::config::env_parse;
use telmoni_shared::middleware::service_auth::{ServiceSecrets, require_service_secret};
use telmoni_shared::seam::{
    AuditEventsQuery, Auth, DocumentCursor, ProjectHome, PurgeHook, SourceDocument,
};
use telmoni_shared::{
    AuthError, FlagSet, OrganizationId, OrganizationStatus, ProjectId, TelmoniError,
};

use cli::{Command, Sweep};

/// The port `telmoni serve` listens on when `PORT` is unset.
pub const DEFAULT_PORT: u16 = 8082;

/// A module a deployment links beside auth and notifications: its routes go
/// under the one listener with the core's layers, its readiness joins the
/// probe, and its loops start with the core's. It reaches the modules beside
/// it through the seams, never through a port.
#[async_trait]
pub trait Module: Send + Sync {
    /// The module's name, in the readiness probe's log line.
    fn name(&self) -> &'static str;

    /// Its routes, gated as the module gates them; the binary adds the
    /// request layers once for every module.
    fn router(&self) -> Router;

    /// The readiness probe: one keyed read of the module's own tables, so a
    /// role missing a grant is a pod that takes no traffic.
    async fn ready(&self) -> sqlx::Result<()>;

    /// Its background loops, started as tasks of the process by `serve`, after
    /// the core's own. Nothing by default.
    fn spawn(&self) {}
}

/// What a binary supplies to build the process: each module's configuration
/// and pool, auth's providers, and the deployment's own purge hook, if any.
pub struct Parts {
    /// Auth's configuration.
    pub auth_config: telmoni_auth::Config,
    /// Auth's pool, opened as the `auth` role.
    pub auth_pool: PgPool,
    /// The issuer, the ways in and the mail transport.
    pub providers: telmoni_auth::Providers,
    /// Notifications' configuration.
    pub notifications_config: telmoni_notifications::Config,
    /// Notifications' pool, opened as the `notifications` role.
    pub notifications_pool: PgPool,
    /// Telemetry's configuration. Required, as its pool is: a binary without
    /// telemetry is not one that can keep a run.
    pub telemetry_config: telmoni_telemetry::Config,
    /// Telemetry's pool, opened as the `telemetry` role.
    pub telemetry_pool: PgPool,
    /// The agent, or `None` for a deployment without one, whose agent lanes
    /// then answer that it is not configured.
    pub agent: Option<AgentParts>,
    /// A module of the deployment's own that holds something of an
    /// organization's outside these tables, run before the organization goes.
    pub purge_hook: Option<Arc<dyn PurgeHook>>,
}

/// The process: every module's state, wired to the others.
pub struct App {
    /// Auth.
    pub auth: Arc<telmoni_auth::AppState>,
    /// Notifications.
    pub notifications: Arc<telmoni_notifications::AppState>,
    /// Telemetry.
    pub telemetry: Arc<telmoni_telemetry::AppState>,
    /// The agent, when the deployment has one.
    pub agent: Option<Arc<telmoni_agent::AppState>>,
    secrets: ServiceSecrets,
    /// The deployment's own modules, in the order they were mounted.
    modules: Vec<Arc<dyn Module>>,
}

/// Auth as notifications, telemetry and the agent hold it before auth is
/// built. Auth holds each of them, so they are handed a slot that is filled a
/// moment later; a call before that answers as an auth that is not there.
#[derive(Default)]
struct LateAuth(OnceLock<Arc<telmoni_auth::AppState>>);

impl LateAuth {
    fn get(&self) -> Result<&Arc<telmoni_auth::AppState>, TelmoniError> {
        self.0
            .get()
            .ok_or_else(|| AuthError::IdentityUnavailable.into())
    }
}

#[async_trait]
impl Auth for LateAuth {
    async fn resolve(&self, headers: &HeaderMap) -> Result<Acting, TelmoniError> {
        self.get()?.resolve(headers).await
    }

    async fn resolve_again(&self, acting: &Acting) -> Result<Acting, TelmoniError> {
        self.get()?.resolve_again(acting).await
    }

    async fn project_homes(
        &self,
        projects: &[ProjectId],
    ) -> Result<Vec<ProjectHome>, TelmoniError> {
        self.get()?.project_homes(projects).await
    }

    async fn organization_slugs(
        &self,
        organizations: &[OrganizationId],
    ) -> Result<Vec<(OrganizationId, String)>, TelmoniError> {
        self.get()?.organization_slugs(organizations).await
    }

    async fn global_flags(&self) -> Result<FlagSet, TelmoniError> {
        self.get()?.global_flags().await
    }

    async fn organization_standing(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<OrganizationStatus>, TelmoniError> {
        self.get()?.organization_standing(organization_id).await
    }

    async fn audit_documents(
        &self,
        after: Option<&DocumentCursor>,
        limit: i64,
    ) -> Result<Vec<SourceDocument>, TelmoniError> {
        self.get()?.audit_documents(after, limit).await
    }

    async fn members(&self, acting: &Acting) -> Result<serde_json::Value, TelmoniError> {
        self.get()?.members(acting).await
    }

    async fn audit_events(
        &self,
        acting: &Acting,
        query: &AuditEventsQuery,
    ) -> Result<serde_json::Value, TelmoniError> {
        self.get()?.audit_events(acting, query).await
    }
}

/// The agent's configuration and pool, when the deployment has one
/// (`AGENT_DATABASE_URL` set).
pub struct AgentParts {
    /// Its configuration.
    pub config: telmoni_agent::Config,
    /// Its pool, opened as the `agent` role.
    pub pool: PgPool,
    /// What records each of the agent's questions
    /// ([`telmoni_shared::seam::AgentObserver`]): the telemetry module, once
    /// it records them; `None` until then.
    pub observer: Option<Arc<dyn telmoni_shared::seam::AgentObserver>>,
}

impl AgentParts {
    /// The agent as the environment describes it, or `None` without one.
    pub async fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(config) = telmoni_agent::Config::from_env()? else {
            return Ok(None);
        };
        let pool = telmoni_agent::boot::open_pool(&config).await?;
        Ok(Some(Self {
            config,
            pool,
            observer: None,
        }))
    }
}

impl App {
    /// Wire the modules together from their parts.
    pub fn assemble(parts: Parts) -> anyhow::Result<Self> {
        let secrets = ServiceSecrets::new(
            parts.auth_config.service_secret.clone(),
            parts.auth_config.service_secret_next.clone(),
        );
        let late_auth = Arc::new(LateAuth::default());
        let notifications = Arc::new(telmoni_notifications::boot::state_from_env(
            parts.notifications_config,
            parts.notifications_pool,
            late_auth.clone(),
        )?);
        let notifier: Arc<dyn telmoni_shared::seam::Notifications> =
            Arc::new(telmoni_notifications::seam::Notifier(notifications.clone()));
        let telemetry = Arc::new(telmoni_telemetry::boot::state(
            parts.telemetry_config,
            parts.telemetry_pool,
            secrets.clone(),
            late_auth.clone(),
        )?);
        let agent = parts
            .agent
            .map(|agent| {
                telmoni_agent::boot::state(
                    agent.config,
                    agent.pool,
                    secrets.clone(),
                    late_auth.clone(),
                    notifier.clone(),
                    agent.observer,
                )
                .map(Arc::new)
            })
            .transpose()?;
        let auth = Arc::new(telmoni_auth::AppState::new(
            parts.auth_config,
            parts.auth_pool,
            parts.providers,
            telmoni_auth::Siblings {
                notifications: Some(notifier),
                telemetry: Some(Arc::new(telmoni_telemetry::seam::TelemetrySeam(
                    telemetry.clone(),
                ))),
                agent: agent.clone().map(|agent| {
                    Arc::new(telmoni_agent::seam::AgentSeam(agent))
                        as Arc<dyn telmoni_shared::seam::Agent>
                }),
                purge_hook: parts.purge_hook,
            },
        ));
        if late_auth.0.set(auth.clone()).is_err() {
            anyhow::bail!("auth was wired twice");
        }
        Ok(Self {
            auth,
            notifications,
            telemetry,
            agent,
            secrets,
            modules: Vec::new(),
        })
    }

    /// Link a module of the deployment's own beside the core's. Built after
    /// [`App::assemble`], so it holds auth and notifications as they are
    /// (`app.auth` is the [`Auth`] seam, `Notifier` the notifications one);
    /// what the core asks of it — a purge hook — went in through
    /// [`Parts`], late-bound on the deployment's side.
    pub fn mount(&mut self, module: Arc<dyn Module>) {
        self.modules.push(module);
    }

    /// The process as the environment describes it: each module's pool as its
    /// own role, auth's providers from the `OIDC_*` and `SMTP_URL` variables
    /// ([`telmoni_auth::boot`]), and no purge hook.
    pub async fn from_env() -> anyhow::Result<Self> {
        let auth_config = telmoni_auth::Config::from_env()?;
        let notifications_config = telmoni_notifications::Config::from_env()?;
        let telemetry_config = telmoni_telemetry::Config::from_env()?;
        let http = telmoni_auth::http_client()?;
        let mail = telmoni_auth::smtp::transport_from_env(&auth_config)?;
        let auth_pool = telmoni_auth::open_pool(&auth_config).await?;
        let notifications_pool =
            telmoni_notifications::boot::open_pool(&notifications_config).await?;
        let telemetry_pool = telmoni_telemetry::boot::open_pool(&telemetry_config).await?;
        let agent = AgentParts::from_env().await?;
        let providers =
            telmoni_auth::boot::providers_from_env(&auth_config, &http, auth_pool.clone(), mail)
                .await?;
        Self::assemble(Parts {
            auth_config,
            auth_pool,
            providers,
            notifications_config,
            notifications_pool,
            telemetry_config,
            telemetry_pool,
            agent,
            purge_hook: None,
        })
    }

    /// Every module's routes under one router, with the health probes and
    /// the request layers added once.
    pub fn router(self: &Arc<Self>) -> Router {
        let probes = Router::new()
            .route("/health", get(health))
            .route("/livez", get(livez))
            .with_state(self.clone());
        let log_level = Router::new()
            .route(
                "/internal/log-level",
                get(telmoni_shared::logging::get_level)
                    .put(telmoni_shared::logging::put_level)
                    .delete(telmoni_shared::logging::reset_level),
            )
            .layer(middleware::from_fn_with_state(
                self.secrets.clone(),
                require_service_secret,
            ));
        let core = Router::new()
            .merge(probes)
            .merge(log_level)
            .merge(telmoni_auth::router(self.auth.clone()))
            .merge(telmoni_notifications::router(self.notifications.clone()))
            .merge(telmoni_telemetry::router(self.telemetry.clone()))
            .merge(match &self.agent {
                Some(agent) => telmoni_agent::router(agent.clone()),
                None => telmoni_agent::absent_router(self.secrets.clone()),
            });
        self.modules
            .iter()
            .fold(core, |router, module| router.merge(module.router()))
            .layer(telmoni_shared::middleware::http::trace_layer())
            .layer(telmoni_shared::middleware::http::request_id_layers())
    }

    /// Run the process until it is told to stop: refuse what a deployed tier
    /// must not run with, start every loop, and serve on `port`.
    /// `RUN_SWEEPS=false` leaves auth's sweeps to `telmoni sweep` runs
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
            if let Some(embedder) = agent.embedder.as_deref() {
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
        if env_parse("RUN_SWEEPS", true)? {
            sweeps::spawn(self.auth.clone());
        } else {
            tracing::warn!(
                "RUN_SWEEPS is false; the deletion, retention and audit sweeps run only as `telmoni sweep`"
            );
        }
        for module in &self.modules {
            module.spawn();
        }

        let addr = format!("0.0.0.0:{port}");
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tracing::info!(addr = %addr, "telmoni listening");

        let serve = axum::serve(listener, self.router())
            .with_graceful_shutdown(telmoni_shared::shutdown::shutdown_signal());
        // ⚠ The delivery loop is raced against the server: a dead loop would
        // leave a Ready pod with every delivery `pending`, so the process
        // exits and the restart is the recovery.
        tokio::select! {
            served = serve => served?,
            ended = delivery => match ended {
                Ok(()) => anyhow::bail!("the delivery loop exited; restarting the process"),
                Err(e) => anyhow::bail!("the delivery loop died: {e}"),
            },
        }
        Ok(())
    }
}

/// Run one command over the process `build` assembles: a binary's whole
/// `main` once the arguments are parsed. `migrate` and `rotate` never build
/// the process — they run as the migrator, against Postgres and ClickHouse
/// alone — and every other command builds it once, with every module the
/// binary mounted, then serves or runs the one sweep.
pub async fn run<F, Fut>(command: Command, build: F) -> anyhow::Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<App>>,
{
    match command {
        Command::Serve => {
            let port: u16 = env_parse("PORT", DEFAULT_PORT)?;
            Arc::new(build().await?).serve(port).await
        }
        Command::Migrate => telmoni_migrator::migrate_from_env().await,
        Command::Rotate => telmoni_migrator::rotate_from_env().await,
        Command::Sweep(sweep) => {
            let app = build().await?;
            tracing::info!(sweep = sweep.as_str(), "sweep starting");
            match sweep {
                Sweep::Deletion => {
                    let swept = telmoni_auth::sweep::deletion(&app.auth).await?;
                    tracing::info!(
                        organizations = swept.organizations,
                        purged = swept.purged,
                        people = swept.people,
                        "deletion sweep complete"
                    );
                }
                Sweep::AuditVerify => telmoni_auth::sweep::audit_verify(&app.auth).await?,
                Sweep::Retention => {
                    telmoni_auth::sweep::retention(&app.auth).await?;
                }
                Sweep::AgentReindex => {
                    let Some(agent) = app.agent.as_ref().filter(|a| a.config.enabled()) else {
                        anyhow::bail!(
                            "agent-reindex needs the agent on: AGENT_DATABASE_URL and \
                             AGENT_MODEL_PROVIDER set"
                        );
                    };
                    // Strict here: a reindex against an endpoint whose width
                    // it cannot learn would only fail row by row.
                    if let Some(embedder) = agent.embedder.as_deref()
                        && telmoni_agent::boot::probe_width(embedder).await?
                            == telmoni_agent::boot::Probe::Unanswered
                    {
                        anyhow::bail!("the embeddings endpoint did not answer; nothing reindexed");
                    }
                    let redone = telmoni_agent::index::reindex(agent).await?;
                    tracing::info!(redone, "agent reindex complete");
                }
                Sweep::AuditExports => {
                    let swept = telmoni_auth::audit_export::sweep(&app.auth).await?;
                    tracing::info!(
                        finished = swept.finished,
                        expired = swept.expired,
                        "audit exports sweep complete"
                    );
                }
            }
            Ok(())
        }
        Command::Terminate(organization) => {
            let app = build().await?;
            let terminated = telmoni_auth::sweep::terminate(&app.auth, &organization).await?;
            tracing::info!(
                organization_id = %organization,
                erase_after = %terminated.erase_after,
                purged = terminated.purged,
                "terminate complete; the deletion sweep finishes it"
            );
            Ok(())
        }
    }
}

/// `GET /health` — **readiness**: every module reaches its own tables, or
/// the process is not ready. Wired to the startup and readiness probes.
async fn health(
    axum::extract::State(app): axum::extract::State<Arc<App>>,
) -> (StatusCode, Json<serde_json::Value>) {
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

/// `GET /livez` — **liveness**. Process-up only, no I/O: a liveness probe that
/// failed on a DB stall would restart every replica at once, and a DB hiccup
/// must never restart the delivery loop.
async fn livez() -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}
