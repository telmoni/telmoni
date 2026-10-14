//! What the suites stand up around the module: the module itself, as its
//! own role, with auth as it holds it — a roster a suite seeds, answered in
//! process; ClickHouse as its two users — the migrator, who applies the file
//! and whom `tenant_isolation` does not hold, and the module's own, whom it
//! does — and a span with nothing in it but what a test sets.
#![allow(dead_code, reason = "each suite uses the part of the harness it needs")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::HeaderMap;
use chrono::{DateTime, SubsecRound, Utc};
use sqlx::PgPool;
use tokio::sync::OnceCell;
use uuid::Uuid;

use telmoni_shared::acting::{Acting, ActingKey, ActingProject};
use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::rbac::project_role;
use telmoni_shared::seam::{
    Auth, Emitted, Entitlements, NoLimits, Notice, Notifications, ProjectHome,
};
use telmoni_shared::test_util::{ServiceRole, chain_lock_held, service_pool};
use telmoni_shared::{
    ApiKeyScope, AuthError, AuthzError, FlagSet, Grants, MonitorCeiling, NotificationKind,
    OrganizationId, OrganizationRole, OrganizationStatus, Paused, ProjectId, Role, SpanKind,
    SpanStatus, TelmoniError, UserId,
};
use telmoni_telemetry::store::{self, SpanRow, Store};
use telmoni_telemetry::{AppState, Config};

/// The service secret the lanes are gated on here.
pub(crate) const SECRET: &str = "test-service-secret";

/// A person's place on one project: its organization, their role there, and
/// their seat on the project, if any.
type Seat = (OrganizationId, OrganizationRole, Option<Role>);

/// Auth as this module holds it: the roster a suite seeds. A bearer is
/// `tok_<user>`, opaque to the module and turned into a role here, as auth
/// would, the project's role from the organization's and the seat's.
#[derive(Default)]
pub(crate) struct AuthStub {
    /// Keyed on `(user, project)`.
    seats: Mutex<HashMap<(String, String), Seat>>,
    /// What the next answer read again gives instead of the roster's.
    again: Mutex<Option<Result<Acting, TelmoniError>>>,
    /// How long the next answer read again takes to arrive.
    again_delay: Mutex<Option<Duration>>,
    /// How many times a request read its answer again.
    read_again: AtomicUsize,
    /// A pool beside the module's, to ask from another session whether the
    /// organization's chain lock is held when an answer is read again.
    chain_probe: Option<PgPool>,
    /// For each answer read again, whether that lock was held.
    chain_held: Mutex<Vec<bool>>,
    /// The API keys auth would resolve, by the bearer that presents them.
    keys: Mutex<HashMap<String, ActingKey>>,
}

impl AuthStub {
    /// Nobody seated.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Nobody seated, and each answer read again asks, from a session of
    /// `pool`'s, whether the organization's chain lock is held: what a change
    /// decided under that lock holds while it asks.
    pub(crate) fn probing(pool: &PgPool) -> Arc<Self> {
        Arc::new(Self {
            chain_probe: Some(pool.clone()),
            ..Self::default()
        })
    }

    /// How many times a request read its answer again.
    pub(crate) fn read_again(&self) -> usize {
        self.read_again.load(Ordering::SeqCst)
    }

    /// Whether the organization's chain lock was held at each answer read
    /// again, in order.
    pub(crate) fn chain_held(&self) -> Vec<bool> {
        self.chain_held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Seat `user` on `organization` at `organization_role`, and on its
    /// `project` at `seat` when they hold one.
    pub(crate) fn seat(
        &self,
        user: &str,
        organization: &OrganizationId,
        project: &ProjectId,
        organization_role: OrganizationRole,
        seat: Option<Role>,
    ) -> &Self {
        self.seats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                (user.to_owned(), project.to_string()),
                (organization.clone(), organization_role, seat),
            );
        self
    }

    /// `token` is a live API key minted on `project`, held by `organization`,
    /// with `scopes`, as auth would resolve it.
    pub(crate) fn key(
        &self,
        token: &str,
        organization: &OrganizationId,
        project: &ProjectId,
        scopes: &[ApiKeyScope],
    ) -> &Self {
        self.keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                token.to_owned(),
                ActingKey {
                    token_id: Uuid::now_v7(),
                    organization_id: organization.clone(),
                    project_id: project.clone(),
                    scopes: scopes.to_vec(),
                },
            );
        self
    }

    /// Hold the next answer read again for `delay`, as an auth whose pool is
    /// spent would.
    pub(crate) fn delaying_next_read_again(&self, delay: Duration) -> &Self {
        *self
            .again_delay
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(delay);
        self
    }

    /// Answer the next request that reads its answer again with `answer`,
    /// once: what auth says when a transfer of the project or of the
    /// organization's ownership, or a deletion, committed between the
    /// request's two reads.
    pub(crate) fn answer_again(&self, answer: Result<Acting, TelmoniError>) -> &Self {
        *self
            .again
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(answer);
        self
    }

    /// `user` on `project`, as the roster stands: the organization the
    /// project is in, never a header's, and the role on the project from
    /// both of the person's.
    fn on_project(&self, user: &str, project: &str) -> Result<Acting, TelmoniError> {
        let user_id = UserId::try_new(user)
            .map_err(|e| AuthError::BadRequest(format!("invalid user id: {e}")))?;
        let seat = self
            .seats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(user.to_owned(), project.to_owned()))
            .cloned();
        let Some((organization_id, organization_role, seat)) = seat else {
            return Err(
                AuthzError::Forbidden("you are not a member of this project".into()).into(),
            );
        };
        let role = project_role(Some(organization_role), seat).ok_or_else(|| {
            AuthzError::Forbidden("you are not a member of this project".to_string())
        })?;
        Ok(Acting {
            user_id,
            organization_id,
            organization_role: Some(organization_role),
            project: Some(ActingProject {
                project_id: ProjectId::try_new(project)
                    .map_err(|e| AuthError::BadRequest(format!("invalid x-project-id: {e}")))?,
                role,
            }),
            session_id: None,
            expires_at: Utc::now().timestamp() + 3600,
        })
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

#[async_trait]
impl Auth for AuthStub {
    async fn resolve_api_key(&self, headers: &HeaderMap) -> Result<ActingKey, TelmoniError> {
        let token = header(headers, "authorization")
            .and_then(|bearer| bearer.strip_prefix("Bearer "))
            .ok_or(AuthError::InvalidToken)?;
        self.keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(token)
            .cloned()
            .ok_or_else(|| AuthError::InvalidToken.into())
    }

    async fn resolve(&self, headers: &HeaderMap) -> Result<Acting, TelmoniError> {
        let user = header(headers, "authorization")
            .and_then(|bearer| bearer.strip_prefix("Bearer tok_"))
            .ok_or(AuthError::Unauthenticated)?;
        let organization = header(headers, "x-organization-id")
            .ok_or_else(|| AuthError::BadRequest("missing x-organization-id header".into()))?;
        if let Some(project) = header(headers, "x-project-id") {
            return self.on_project(user, project);
        }

        let user_id = UserId::try_new(user)
            .map_err(|e| AuthError::BadRequest(format!("invalid user id: {e}")))?;
        let seats = self
            .seats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((organization_id, organization_role, _)) = seats
            .iter()
            .find(|((u, _), (o, _, _))| u == user && o.as_str() == organization)
            .map(|(_, seat)| seat.clone())
        else {
            return Err(
                AuthzError::Forbidden("you are not a member of this organization".into()).into(),
            );
        };
        Ok(Acting {
            user_id,
            organization_id,
            organization_role: Some(organization_role),
            project: None,
            session_id: None,
            expires_at: Utc::now().timestamp() + 3600,
        })
    }

    async fn resolve_again(&self, acting: &Acting) -> Result<Acting, TelmoniError> {
        self.read_again.fetch_add(1, Ordering::SeqCst);
        if let Some(pool) = &self.chain_probe {
            let held = chain_lock_held(pool, &acting.organization_id).await;
            self.chain_held
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(held);
        }
        let delay = self
            .again_delay
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let scripted = self
            .again
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(answer) = scripted {
            return answer;
        }
        let project = acting.project_or_bad_request()?;
        self.on_project(acting.user_id.as_str(), project.project_id.as_str())
    }

    async fn global_flags(&self) -> Result<FlagSet, TelmoniError> {
        Ok(FlagSet::all_on())
    }

    /// Where each project is held, from the roster: the organization of any
    /// seat on it, and its ids for its slugs.
    async fn project_homes(
        &self,
        projects: &[ProjectId],
    ) -> Result<Vec<ProjectHome>, TelmoniError> {
        let seats = self
            .seats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(projects
            .iter()
            .filter_map(|project| {
                seats
                    .iter()
                    .find(|((_, p), _)| *p == project.to_string())
                    .map(|(_, (organization, _, _))| ProjectHome {
                        project_id: project.clone(),
                        slug: project.to_string(),
                        organization_id: organization.clone(),
                        organization_slug: organization.to_string(),
                    })
            })
            .collect())
    }

    async fn organization_standing(
        &self,
        _organization_id: &OrganizationId,
    ) -> Result<Option<OrganizationStatus>, TelmoniError> {
        Ok(Some(OrganizationStatus::Active))
    }
}

/// The module over `pool`, connected as its own role, with `auth` answering
/// who acts. ClickHouse is named and never dialled: nothing a suite that
/// reads no span asks of it.
pub(crate) fn state(pool: &PgPool, auth: Arc<dyn Auth>) -> Arc<AppState> {
    state_with_entitlements(pool, auth, Arc::new(NoLimits))
}

/// [`state`], with `entitlements` answering what each organization may take
/// in, as a deployment that sells plans answers it.
pub(crate) fn state_with_entitlements(
    pool: &PgPool,
    auth: Arc<dyn Auth>,
    entitlements: Arc<dyn Entitlements>,
) -> Arc<AppState> {
    // Nothing listens on either: the replica stands in for Redis, as it does
    // on a laptop with none, and a lane that reaches for ClickHouse fails.
    state_with_notices(pool, auth, entitlements, NoticeLog::new())
}

/// Entitlements under which one organization is paused, as a deployment that
/// sells plans holds an organization it will not take anything new for.
pub(crate) struct PausedOrganization(pub(crate) OrganizationId);

#[async_trait]
impl Entitlements for PausedOrganization {
    async fn grants(&self, organization_id: &OrganizationId) -> Result<Grants, TelmoniError> {
        if *organization_id != self.0 {
            return Ok(Grants::unlimited());
        }
        Ok(Grants {
            paused: Some(Paused {
                entitlement: "pro".into(),
                detail: "This organization is paused.".into(),
            }),
        })
    }
}

/// Entitlements under which every organization holds `limit` monitors,
/// answered after `after`, with whether the organization's chain lock was
/// held at each ask recorded in order, probed from `pool` as auth's stub
/// probes it: the lanes read the ceiling before the lock, never under it.
pub(crate) struct Ceiling {
    pool: PgPool,
    limit: u32,
    after: Duration,
    asked: Mutex<Vec<bool>>,
}

impl Ceiling {
    pub(crate) fn of(pool: &PgPool, limit: u32) -> Arc<Self> {
        Self::answered_after(pool, limit, Duration::ZERO)
    }

    pub(crate) fn answered_after(pool: &PgPool, limit: u32, after: Duration) -> Arc<Self> {
        Arc::new(Self {
            pool: pool.clone(),
            limit,
            after,
            asked: Mutex::new(Vec::new()),
        })
    }

    /// Whether the organization's chain lock was held at each ask, in order.
    pub(crate) fn asked(&self) -> Vec<bool> {
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl Entitlements for Ceiling {
    async fn grants(&self, organization_id: &OrganizationId) -> Result<Grants, TelmoniError> {
        let _ = organization_id;
        Ok(Grants::unlimited())
    }

    async fn monitor_ceiling(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<MonitorCeiling>, TelmoniError> {
        let held = chain_lock_held(&self.pool, organization_id).await;
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(held);
        tokio::time::sleep(self.after).await;
        Ok(Some(MonitorCeiling {
            limit: self.limit,
            entitlement: "pro".into(),
            detail: "Move to Pro for more.".into(),
        }))
    }
}

/// One notice as the module handed it through the seam.
#[derive(Debug, Clone)]
pub(crate) struct LoggedNotice {
    pub(crate) feed_id: Uuid,
    pub(crate) organization_id: OrganizationId,
    pub(crate) project_id: Option<ProjectId>,
    pub(crate) kind: NotificationKind,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) metadata: serde_json::Value,
    pub(crate) dedup_key: Option<String>,
}

/// How the fake answers a notice, where a suite has it fail: as notifications
/// refusing one, or unable to write one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Answer {
    #[default]
    Taken,
    Refused,
    Down,
}

/// Notifications as this module holds it: every notice handed through the
/// seam, kept for a suite to read, and a replayed dedup key answered with
/// its first row and kept once, as the module answers one.
#[derive(Default)]
pub(crate) struct NoticeLog {
    notices: Mutex<Vec<LoggedNotice>>,
    answer: Mutex<Answer>,
}

impl NoticeLog {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// How every notice from now on is answered.
    pub(crate) fn answer(&self, answer: Answer) {
        *self
            .answer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = answer;
    }

    /// Every notice handed over, in order.
    pub(crate) fn notices(&self) -> Vec<LoggedNotice> {
        self.notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl Notifications for NoticeLog {
    async fn emit(
        &self,
        organization_id: &OrganizationId,
        project_id: Option<&ProjectId>,
        notice: Notice<'_>,
    ) -> Result<Emitted, TelmoniError> {
        match *self
            .answer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            Answer::Taken => {}
            Answer::Refused => {
                return Err(TelmoniError::BadRequest("the fake refuses notices".into()));
            }
            Answer::Down => return Err(TelmoniError::Internal("the fake is down".into())),
        }
        let mut notices = self
            .notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(key) = notice.dedup_key
            && let Some(first) = notices
                .iter()
                .find(|logged| logged.dedup_key.as_deref() == Some(key))
        {
            return Ok(Emitted {
                feed_id: first.feed_id,
                deduplicated: true,
            });
        }
        let feed_id = Uuid::new_v4();
        notices.push(LoggedNotice {
            feed_id,
            organization_id: organization_id.clone(),
            project_id: project_id.cloned(),
            kind: notice.kind,
            title: notice.title.to_owned(),
            body: notice.body.to_owned(),
            metadata: notice.metadata,
            dedup_key: notice.dedup_key.map(str::to_owned),
        });
        Ok(Emitted {
            feed_id,
            deduplicated: false,
        })
    }

    async fn purge_organization(
        &self,
        _organization_id: &OrganizationId,
    ) -> Result<u64, TelmoniError> {
        Ok(0)
    }

    async fn purge_project(&self, _project_id: &ProjectId) -> Result<u64, TelmoniError> {
        Ok(0)
    }

    async fn redact_person(&self, _user_id: &UserId) -> Result<u64, TelmoniError> {
        Ok(0)
    }
}

/// [`state_with_entitlements`], with `notices` keeping every notice the
/// module hands through the seam.
pub(crate) fn state_with_notices(
    pool: &PgPool,
    auth: Arc<dyn Auth>,
    entitlements: Arc<dyn Entitlements>,
    notices: Arc<NoticeLog>,
) -> Arc<AppState> {
    state_at(
        pool,
        auth,
        "http://telemetry:telemetry@127.0.0.1:1",
        "redis://127.0.0.1:1",
        entitlements,
        notices,
    )
}

/// The module over `pool`, with the stores a suite can reach: ClickHouse as
/// `TELEMETRY_CLICKHOUSE_URL` names it, which the caller has checked with
/// [`clickhouse_or_skip`], and Redis as `REDIS_URL` names it, or the compose
/// file's; neither dialled until a lane needs it.
pub(crate) fn state_with_stores(pool: &PgPool, auth: Arc<dyn Auth>) -> Arc<AppState> {
    state_with_stores_and_notices(pool, auth, NoticeLog::new())
}

/// [`state_with_stores`], with `notices` keeping every notice the module
/// hands through the seam.
pub(crate) fn state_with_stores_and_notices(
    pool: &PgPool,
    auth: Arc<dyn Auth>,
    notices: Arc<NoticeLog>,
) -> Arc<AppState> {
    let clickhouse = std::env::var("TELEMETRY_CLICKHOUSE_URL").expect(
        "state_with_stores: TELEMETRY_CLICKHOUSE_URL is set, checked by clickhouse_or_skip",
    );
    let redis = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_owned());
    state_at(pool, auth, &clickhouse, &redis, Arc::new(NoLimits), notices)
}

fn state_at(
    pool: &PgPool,
    auth: Arc<dyn Auth>,
    clickhouse_url: &str,
    redis_url: &str,
    entitlements: Arc<dyn Entitlements>,
    notifications: Arc<dyn Notifications>,
) -> Arc<AppState> {
    let config = Config {
        database_url: "".into(),
        clickhouse_url: clickhouse_url.into(),
        // A laptop's key: the suites seal and open on their own machine, and
        // name no KMS.
        telemetry_kek: format!("local:{}", "0".repeat(64)).into(),
        kms_api_base: "http://127.0.0.1:1".into(),
        metadata_api_base: "http://127.0.0.1:1".into(),
        redis_url: redis_url.into(),
    };
    Arc::new(
        telmoni_telemetry::boot::state(
            config,
            service_pool(pool, ServiceRole::Telemetry),
            ServiceSecrets::new(SECRET, None::<String>),
            auth,
            entitlements,
            notifications,
        )
        .expect("the module's state over a laptop's key"),
    )
}

/// ClickHouse as both of the users a tier runs it with.
pub(crate) struct ClickHouse {
    /// The migrator's user, whom `tenant_isolation` does not hold.
    pub(crate) migrator: clickhouse::Client,
    /// The module's user, through the one query module.
    pub(crate) store: Store,
    /// The one query module as the migrator's user: a server whose one user
    /// made the tables and reads them, as the Compose quickstart's does,
    /// where `tenant_isolation` holds nobody.
    pub(crate) unheld: Store,
    /// The module's user, bare: a query the query module would never send.
    pub(crate) telemetry: clickhouse::Client,
}

/// The file applied once per suite, as `telmoni migrate` applies it.
static MIGRATED: OnceCell<()> = OnceCell::const_new();

/// ClickHouse as `MIGRATOR_CLICKHOUSE_URL` and `TELEMETRY_CLICKHOUSE_URL` name
/// its two users, with the file applied; or `None`, with a notice, when
/// either is unset. `make test` and CI set both.
pub(crate) async fn clickhouse_or_skip(test: &str) -> Option<ClickHouse> {
    let url = |var: &str| std::env::var(var).ok().filter(|v| !v.trim().is_empty());
    let (Some(migrator), Some(telemetry)) = (
        url("MIGRATOR_CLICKHOUSE_URL"),
        url("TELEMETRY_CLICKHOUSE_URL"),
    ) else {
        eprintln!(
            "skipping {test}: MIGRATOR_CLICKHOUSE_URL or TELEMETRY_CLICKHOUSE_URL unset \
             (`make docker-up` starts a local ClickHouse; `make test` sets both)"
        );
        return None;
    };
    let unheld = Store::connect(&migrator).expect("MIGRATOR_CLICKHOUSE_URL is a ClickHouse URL");
    let migrator = store::client(&migrator).expect("MIGRATOR_CLICKHOUSE_URL is a ClickHouse URL");
    MIGRATED
        .get_or_init(|| async {
            telmoni_telemetry::schema::apply(&migrator)
                .await
                .expect("the ClickHouse file applies as the migrator");
        })
        .await;
    Some(ClickHouse {
        migrator,
        store: Store::connect(&telemetry).expect("TELEMETRY_CLICKHOUSE_URL is a ClickHouse URL"),
        unheld,
        telemetry: store::client(&telemetry).expect("TELEMETRY_CLICKHOUSE_URL is a ClickHouse URL"),
    })
}

/// Wait for `holds` to answer true, asking every 50 ms for up to ten
/// seconds, or fail naming `what`: a lightweight delete answers once it is
/// recorded, and its rows go a moment later.
pub(crate) async fn eventually<F, Fut>(what: &str, mut holds: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if holds().await {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{what}: still not so after ten seconds"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A project's rows in both tables, as the module's user reads them: its
/// spans, and the hours of its totals.
pub(crate) async fn rows_of(store: &Store, project: &ProjectId) -> (u64, u64) {
    let only = store::Projects::one(project.clone());
    let spans: u64 = store
        .read(
            &only,
            "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_one()
        .await
        .expect("count a project's spans");
    let hours: u64 = store
        .read(
            &only,
            "SELECT count() FROM telemetry.spans_hourly \
             WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_one()
        .await
        .expect("count a project's hours of totals");
    (spans, hours)
}

/// One run of `support-bot` as the users and sessions pages sum it: its run
/// span, one step and one model call with `tokens`, every span carrying
/// `user` and `session`, started at `started` and received now, the run
/// `failed` or not, its span ids from `n`.
#[expect(
    clippy::too_many_arguments,
    reason = "eight facts name a run; a struct built at each of the eleven call sites would say the same in more lines"
)]
pub(crate) fn run_of(
    project: &ProjectId,
    run: Uuid,
    user: &str,
    session: &str,
    started: DateTime<Utc>,
    tokens: (u64, u64),
    failed: bool,
    n: u64,
) -> [SpanRow; 3] {
    let ids = |row: SpanRow, span: u64, parent: Option<u64>| SpanRow {
        run_id: run,
        agent: "support-bot".into(),
        span_id: format!("{span:016x}"),
        parent_span_id: parent.map(|p| format!("{p:016x}")).unwrap_or_default(),
        user_id: user.into(),
        session_id: session.into(),
        ..row
    };
    [
        ids(
            SpanRow {
                started_at: started,
                ended_at: started + chrono::Duration::seconds(2),
                status: if failed {
                    SpanStatus::Error
                } else {
                    SpanStatus::Ok
                },
                error_type: if failed {
                    "RuntimeError".into()
                } else {
                    String::new()
                },
                ..span(project, SpanKind::Run, "support-bot", 2_000)
            },
            n,
            None,
        ),
        ids(
            SpanRow {
                started_at: started + chrono::Duration::milliseconds(10),
                ended_at: started + chrono::Duration::milliseconds(1_900),
                ..span(project, SpanKind::Step, "answer", 1_890)
            },
            n + 1,
            Some(n),
        ),
        ids(
            SpanRow {
                started_at: started + chrono::Duration::milliseconds(20),
                ended_at: started + chrono::Duration::seconds(1),
                model: "claude-sonnet-5-5".into(),
                provider: "anthropic".into(),
                input_tokens: tokens.0,
                output_tokens: tokens.1,
                ..span(project, SpanKind::ModelCall, "chat claude-sonnet-5-5", 980)
            },
            n + 2,
            Some(n + 1),
        ),
    ]
}

/// A finished span of `kind`, received now, lasting `millis`, with nothing
/// else set.
pub(crate) fn span(project_id: &ProjectId, kind: SpanKind, name: &str, millis: i64) -> SpanRow {
    let now = Utc::now();
    received(project_id, kind, name, millis, now)
}

/// [`span`], received at `at`, to the microsecond the table keeps.
pub(crate) fn received(
    project_id: &ProjectId,
    kind: SpanKind,
    name: &str,
    millis: i64,
    at: DateTime<Utc>,
) -> SpanRow {
    let at = at.trunc_subsecs(6);
    SpanRow {
        project_id: project_id.clone(),
        received_at: at,
        units: 1,
        run_id: Uuid::new_v4(),
        agent: "nightly-report".into(),
        trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
        span_id: "00f067aa0ba902b7".into(),
        parent_span_id: String::new(),
        kind,
        name: name.into(),
        started_at: at - chrono::Duration::milliseconds(millis),
        ended_at: at,
        status: SpanStatus::Ok,
        error_type: String::new(),
        model: String::new(),
        provider: String::new(),
        input_tokens: 0,
        output_tokens: 0,
        cache_read_input_tokens: 0,
        cache_write_input_tokens: 0,
        user_id: String::new(),
        session_id: String::new(),
        tags: Vec::new(),
        bound_stall_after_s: None,
        bound_max_steps: None,
        bound_max_cost_usd: None,
        sample_rate: None,
        attributes: Vec::new(),
    }
}
