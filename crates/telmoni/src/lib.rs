//! The Telmoni server: one process, and the modules it is made of.
//!
//! Auth, notifications, telemetry and the agent are libraries. This crate
//! links them, hands each the others through the seams in
//! [`telmoni_shared::seam`], mounts their routers under one listener, and
//! runs their background loops — the delivery loop, telemetry's purge sweep,
//! the retention sweeps, the agent's indexer, the deletion sweep and the
//! nightly audit walk — as tasks of the same process. Each module still connects as its own database
//! role, so row-level security and the grants stand exactly as they did
//! across services.
//!
//! The binary is the command line over this: `telmoni-server serve`, and the
//! migrator and each sweep as a subcommand run as a Job. A binary built for
//! another deployment calls [`App::assemble`] with its own identity
//! provider and transport, [`App::mount`]s the modules of its own, and hands
//! the same [`run`] the command it parsed.

#![deny(missing_docs)]

pub mod cli;
pub mod door;
mod serve;
pub mod sweeps;

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::http::{HeaderMap, StatusCode};
use axum::{Json, Router, middleware, routing::get};
use serde_json::json;
use sqlx::PgPool;

use telmoni_shared::acting::{Acting, ActingKey};
use telmoni_shared::config::env_parse;
use telmoni_shared::heartbeat::Heartbeats;
use telmoni_shared::middleware::service_auth::{ServiceSecrets, require_service_secret};
use telmoni_shared::seam::{
    AuditEventsQuery, Auth, DocumentCursor, Entitlements, NoLimits, OwnedOrganization,
    OwnershipHook, ProjectHome, PurgeHook, ReadableProject, SourceDocument,
};
use telmoni_shared::{
    AuthError, FlagSet, OrganizationId, OrganizationStatus, ProjectId, TelmoniError, UserId,
};

use cli::{Command, Sweep};

/// The port `telmoni-server serve` listens on when `PORT` is unset.
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

    /// Beside readiness, for the watchdog: its loops no replica has beaten
    /// for three of their periods, as `telmoni_shared::heartbeat` reads them.
    /// None by default.
    async fn stale(&self) -> Vec<String> {
        Vec::new()
    }

    /// Its background loops, started as tasks of the process by `serve`, after
    /// the core's own. Nothing by default.
    fn spawn(&self) {}
}

/// What a binary supplies to build the process: each module's configuration
/// and pool, auth's providers, and the deployment's own hooks and
/// entitlements, if any.
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
    /// A module of the deployment's own whose answer depends on who owns an
    /// organization: asked before one changes hands, and told when one is
    /// founded or does. `None` asks nobody.
    pub ownership_hook: Option<Arc<dyn OwnershipHook>>,
    /// What each organization may take in, answered by a deployment that
    /// sells plans; `None` is [`NoLimits`], everything for everybody.
    pub entitlements: Option<Arc<dyn Entitlements>>,
    /// How many `X-Forwarded-For` entries the proxies in front of the server
    /// append (`TRUSTED_PROXY_HOPS`), which the machine door's per-address
    /// limits read the caller's address past ([`door`]).
    pub trusted_proxy_hops: u8,
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
    /// The machine door, in front of `/v1`, `/cli/v1` and `/otlp`.
    door: Arc<door::Door>,
    secrets: ServiceSecrets,
    /// The deployment's own modules, in the order they were mounted.
    modules: Vec<Arc<dyn Module>>,
    /// Auth's sweeps, which this process runs on its timers, and their beats.
    sweeps: Arc<Heartbeats<telmoni_auth::db::AuthLane>>,
    /// The watchdog's last answer on this replica, and when it was read.
    watchdog: tokio::sync::Mutex<Option<(Instant, Vec<String>)>>,
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

    async fn resolve_api_key(&self, headers: &HeaderMap) -> Result<ActingKey, TelmoniError> {
        self.get()?.resolve_api_key(headers).await
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

    async fn readable_projects(
        &self,
        acting: &Acting,
    ) -> Result<Vec<ReadableProject>, TelmoniError> {
        self.get()?.readable_projects(acting).await
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

    async fn organizations_owned_by(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<OwnedOrganization>, TelmoniError> {
        self.get()?.organizations_owned_by(user_id).await
    }

    async fn owner_of(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<UserId>, TelmoniError> {
        self.get()?.owner_of(organization_id).await
    }

    async fn owners_of_several(&self) -> Result<Vec<UserId>, TelmoniError> {
        self.get()?.owners_of_several().await
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
}

impl AgentParts {
    /// The agent as the environment describes it, or `None` without one.
    pub async fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(config) = telmoni_agent::Config::from_env()? else {
            return Ok(None);
        };
        let pool = telmoni_agent::boot::open_pool(&config).await?;
        Ok(Some(Self { config, pool }))
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
        let entitlements: Arc<dyn Entitlements> =
            parts.entitlements.unwrap_or_else(|| Arc::new(NoLimits));
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
            entitlements.clone(),
            notifier.clone(),
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
                    entitlements.clone(),
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
                ownership_hook: parts.ownership_hook,
                entitlements: Some(entitlements),
            },
        ));
        if late_auth.0.set(auth.clone()).is_err() {
            anyhow::bail!("auth was wired twice");
        }
        let door = door::Door::new(telemetry.clone(), parts.trusted_proxy_hops);
        Ok(Self {
            auth,
            notifications,
            telemetry,
            agent,
            door,
            secrets,
            modules: Vec::new(),
            sweeps: Arc::new(Heartbeats::new("auth", telmoni_auth::db::AuthLane)),
            watchdog: tokio::sync::Mutex::new(None),
        })
    }

    /// Link a module of the deployment's own beside the core's. Built after
    /// [`App::assemble`], so it holds auth and notifications as they are
    /// (`app.auth` is the [`Auth`] seam, `Notifier` the notifications one);
    /// what the core asks of it — a purge hook, an ownership hook, the
    /// entitlements — went in through [`Parts`], late-bound on the
    /// deployment's side.
    pub fn mount(&mut self, module: Arc<dyn Module>) {
        self.modules.push(module);
    }

    /// The process as the environment describes it: each module's pool as its
    /// own role, auth's providers from the `AUTH_*` and `SMTP_URL` variables
    /// ([`telmoni_auth::boot`]), and no hook of a deployment's own: nothing
    /// purged beside the core's modules, nothing asked of a handover, and no
    /// limits.
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
            ownership_hook: None,
            entitlements: None,
            trusted_proxy_hops: env_parse("TRUSTED_PROXY_HOPS", 2_u8)?,
        })
    }

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
        let serve = serve::serve(
            listener,
            self.router(),
            serve::HEAD_WAIT,
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
const ALERTS_STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Run one command over the process `build` assembles: a binary's whole
/// `main` once the arguments are parsed. `migrate` and `rotate` never build
/// the process — they run as the migrator, against Postgres and ClickHouse
/// alone — and every other command builds it once, with every module the
/// binary mounted, then serves or does its one thing.
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
                Sweep::Auth(sweep) => sweeps::once(&app.auth, sweep).await?,
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
/// the process is not ready. The chart's readiness probe; `/livez` is its liveness.
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

/// How long a replica's answer to the watchdog stands: the stores are read at
/// most this often, however often `/heartbeat` is asked.
const HEARTBEAT_HELD_FOR: Duration = Duration::from_secs(5);

/// `GET /heartbeat` — the **external watchdog's**, on the machine host: `200`,
/// or `503` naming each loop of the process that no replica has beaten for
/// three of its periods, and nothing else. Never a probe of the chart's: a
/// stopped loop is a page, not a restart.
async fn heartbeat(
    axum::extract::State(app): axum::extract::State<Arc<App>>,
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
async fn livez() -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

/// A path no lane serves: `404` as a problem document, as each lane answers
/// one for a path under it, so a client reads one shape from the server
/// wherever it knocks.
async fn not_found() -> axum::response::Response {
    telmoni_shared::middleware::problem::not_found("no such lane")
}
