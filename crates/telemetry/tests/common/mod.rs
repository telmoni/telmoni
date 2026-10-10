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

use telmoni_shared::acting::{Acting, ActingProject};
use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::rbac::project_role;
use telmoni_shared::seam::Auth;
use telmoni_shared::test_util::{ServiceRole, chain_lock_held, service_pool};
use telmoni_shared::{
    AuthError, AuthzError, FlagSet, OrganizationId, OrganizationRole, OrganizationStatus,
    ProjectId, Role, SpanKind, SpanStatus, TelmoniError, UserId,
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
    Arc::new(AppState {
        db: service_pool(pool, ServiceRole::Telemetry),
        config: Config {
            database_url: "".into(),
            clickhouse_url: "http://telemetry:telemetry@127.0.0.1:1".into(),
        },
        store: Store::connect("http://telemetry:telemetry@127.0.0.1:1").expect("a ClickHouse URL"),
        service_secrets: ServiceSecrets::new(SECRET, None::<String>),
        auth,
    })
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
        customer_id: String::new(),
        conversation_id: String::new(),
        tags: Vec::new(),
        bound_stall_after_s: None,
        bound_max_steps: None,
        bound_max_cost_usd: None,
        sample_rate: None,
        attributes: Vec::new(),
    }
}
