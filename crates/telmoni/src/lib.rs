//! The Telmoni server: one process, and the modules it is made of.
//!
//! Auth, notifications, telemetry and the agent are libraries. This crate
//! links them, hands each the others through the seams in
//! [`telmoni_shared::seam`], mounts their routers under one listener, and
//! runs their background loops — notifications' delivery and retention,
//! telemetry's purge sweep, alerts and plan notices, the agent's indexer, and
//! auth's deletion, retention, audit and export sweeps — as tasks of the same
//! process. Each module still connects as its own database role, so row-level
//! security and the grants stand exactly as they did across services.
//!
//! The binary is the command line over this: `telmoni-server serve`, and the
//! migrator and each sweep as a subcommand run as a Job. A binary built for
//! another deployment calls [`App::assemble`] with its own identity
//! provider and transport, [`App::mount`]s the modules of its own, and hands
//! the same [`run`] the command it parsed.

#![deny(missing_docs)]

mod accept;
pub mod command;
pub mod door;
mod late_auth;
mod probes;
mod serve;
pub mod sweeps;

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use axum::Router;
use sqlx::PgPool;

use telmoni_shared::config::env_parse;
use telmoni_shared::heartbeat::Heartbeats;
use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::seam::{Entitlements, NoLimits, OwnershipHook, PurgeHook};

use command::{Command, Sweep};
use late_auth::LateAuth;

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

impl Parts {
    /// Every part as the environment describes it, with a deployment's own
    /// mail transport, built from auth's configuration, and its own external
    /// provider when it brings one in place of the one
    /// `AUTH_CUSTOM_*` would name. No hook: a deployment sets its own.
    ///
    /// # Errors
    ///
    /// A variable missing or malformed, a pool that will not open, or the
    /// transport's own refusal.
    pub async fn from_env_with(
        mail: impl FnOnce(
            &telmoni_auth::Config,
        ) -> anyhow::Result<Arc<dyn telmoni_shared::mail::MailSender>>,
        external: Option<telmoni_auth::external::External>,
    ) -> anyhow::Result<Self> {
        let auth_config = telmoni_auth::Config::from_env()?;
        let notifications_config = telmoni_notifications::Config::from_env()?;
        let telemetry_config = telmoni_telemetry::Config::from_env()?;
        let http = telmoni_auth::http_client()?;
        let mail = mail(&auth_config)?;
        let auth_pool = telmoni_auth::open_pool(&auth_config).await?;
        let notifications_pool =
            telmoni_notifications::boot::open_pool(&notifications_config).await?;
        let telemetry_pool = telmoni_telemetry::boot::open_pool(&telemetry_config).await?;
        let agent = AgentParts::from_env().await?;
        let providers = match external {
            Some(external) => {
                telmoni_auth::boot::providers_with(
                    &auth_config,
                    auth_pool.clone(),
                    mail,
                    Some(external),
                )
                .await?
            }
            None => {
                telmoni_auth::boot::providers_from_env(&auth_config, &http, auth_pool.clone(), mail)
                    .await?
            }
        };
        Ok(Self {
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
            secrets.clone(),
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
        late_auth.fill(auth.clone())?;
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
    /// (`app.auth` is the [`Auth`](telmoni_shared::seam::Auth) seam, `Notifier` the notifications one);
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
        Self::assemble(Parts::from_env_with(telmoni_auth::smtp::transport_from_env, None).await?)
    }

    /// The service secrets every module's internal lanes are gated by, for a
    /// deployment's own module mounted beside them.
    #[must_use]
    pub fn service_secrets(&self) -> &ServiceSecrets {
        &self.secrets
    }
}

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
                Sweep::AgentReindex => sweeps::agent_reindex(&app).await?,
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
