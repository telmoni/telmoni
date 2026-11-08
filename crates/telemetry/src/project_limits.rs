//! What an organization's owners and admins hold one project to of the pool
//! its plan gives them all (`telemetry.project_limits`): spans a month,
//! content held, alerts, monitors, objectives and alert deliveries a day.
//! Each cap is at most the plan's, and one unset shares the pool as before.
//! Past one, that project alone is held as the organization is past its
//! plan: its spans sampled with no grace, since the cap is the team's own,
//! its content kept as metadata alone, a new alert, monitor or objective
//! refused and a starter not made, its deliveries held to the feed until the
//! day turns. A refusal names the cap and who may raise it, never the plan,
//! and the owner and admins are told once in the feed. Read at each door
//! beside the organization's own number, kept a minute a project.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::json;

use telmoni_shared::db::tenant_session::{HasProject, Scoped, maintenance_scope};
use telmoni_shared::seam::{self, OrganizationAlert};
use telmoni_shared::{OrganizationId, PlanError, ProjectId, TelmoniError};

use crate::AppState;
use crate::db::TelemetryLane;
use crate::handler::chain;

/// How long a project's caps are kept: one changed holds within a minute.
const KEEP_FOR: Duration = Duration::from_secs(60);

/// How many projects' caps are kept at once.
const KNOWN_MAX: usize = 10_000;

/// The `entitlement` of a refusal a project's own cap makes: its
/// organization's owners and admins lift it, never a plan.
pub const PROJECT_LIMIT: &str = "project_limit";

/// One project's caps; `None` shares the organization's pool whole.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
#[serde(deny_unknown_fields)]
pub struct Caps {
    pub spans: Option<i64>,
    pub content_bytes: Option<i64>,
    pub alerts: Option<i32>,
    pub monitors: Option<i32>,
    pub objectives: Option<i32>,
    pub alert_deliveries_per_day: Option<i32>,
}

impl Caps {
    /// The spans a month it may keep whole, if capped.
    #[must_use]
    pub fn spans(&self) -> Option<u64> {
        self.spans.and_then(|n| u64::try_from(n).ok())
    }

    /// The content it may hold, in bytes, if capped.
    #[must_use]
    pub fn content_bytes(&self) -> Option<u64> {
        self.content_bytes.and_then(|n| u64::try_from(n).ok())
    }

    /// The notices its alerts may send a UTC day, if capped.
    #[must_use]
    pub fn alert_deliveries_per_day(&self) -> Option<u64> {
        self.alert_deliveries_per_day
            .and_then(|n| u64::try_from(n).ok())
    }
}

/// What the project may hold of a counted thing, made one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counted {
    Alerts,
    Monitors,
    Objectives,
}

impl Counted {
    fn cap(self, caps: &Caps) -> Option<u64> {
        let cap = match self {
            Self::Alerts => caps.alerts,
            Self::Monitors => caps.monitors,
            Self::Objectives => caps.objectives,
        };
        cap.and_then(|n| u64::try_from(n).ok())
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Alerts => "alerts",
            Self::Monitors => "monitors",
            Self::Objectives => "objectives",
        }
    }
}

/// The sentence a project past its cap on `what` is refused or told with.
#[must_use]
pub fn detail(what: &str, cap: u64) -> String {
    format!(
        "This project's cap is {cap} {what}, its share of the organization's; an owner or admin \
         of the organization may raise it on its Limits page."
    )
}

/// Whether one more `what` may be made in the project, with `held` already
/// held. Refused, as a 402 its organization's owners and admins lift, at the
/// cap.
pub fn admit(caps: &Caps, what: Counted, held: u64) -> Result<(), TelmoniError> {
    match what.cap(caps) {
        Some(cap) if held >= cap => Err(PlanError::LimitReached {
            entitlement: PROJECT_LIMIT.into(),
            detail: detail(what.noun(), cap),
        }
        .into()),
        _ => Ok(()),
    }
}

/// Each project's caps as one replica holds them, a minute at a time.
#[derive(Default)]
pub struct Known {
    known: Mutex<HashMap<ProjectId, (Caps, Instant)>>,
}

impl Known {
    /// The project's caps, from memory for a minute after they were read; a
    /// read that fails is no cap, as an outage of ours never holds a project
    /// back.
    pub async fn of(&self, state: &AppState, project_id: &ProjectId) -> Caps {
        if let Some((caps, at)) = self
            .known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(project_id)
            && at.elapsed() < KEEP_FOR
        {
            return *caps;
        }
        let read = async {
            let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
            let caps = caps_of(&mut lane, project_id).await?;
            lane.commit().await?;
            Ok::<_, sqlx::Error>(caps)
        };
        let caps = match read.await {
            Ok(caps) => caps,
            Err(error) => {
                tracing::warn!(project_id = %project_id, error = %error,
                    "telemetry: a project's caps could not be read; none is held");
                return Caps::default();
            }
        };
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        if known.len() >= KNOWN_MAX {
            known.retain(|_, (_, at)| at.elapsed() < KEEP_FOR);
        }
        known.insert(project_id.clone(), (caps, Instant::now()));
        caps
    }

    /// The project's caps forgotten, so the replica that set them holds them
    /// at once.
    pub fn forget(&self, project_id: &ProjectId) {
        self.known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(project_id);
    }
}

/// A project's caps as its row keeps them; none where it has no row, or its
/// row was set in an organization it has since moved out of.
pub async fn caps_of<B: telmoni_shared::db::tenant_session::Binding>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Caps> {
    Ok(sqlx::query_as::<_, Caps>(
        "SELECT l.spans, l.content_bytes, l.alerts, l.monitors, l.objectives,
                l.alert_deliveries_per_day
           FROM telemetry.project_limits AS l
           JOIN telemetry.project_settings AS s
             ON s.project_id = l.project_id AND s.organization_id = l.organization_id
          WHERE l.project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?
    .unwrap_or_default())
}

/// Every capped project of the organization, by id.
pub async fn caps_in<B: telmoni_shared::db::tenant_session::Binding>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
) -> sqlx::Result<HashMap<String, Caps>> {
    let found: Vec<CapsRow> = sqlx::query_as(
        "SELECT project_id, spans, content_bytes, alerts, monitors, objectives,
                alert_deliveries_per_day
           FROM telemetry.project_limits WHERE organization_id = $1",
    )
    .bind(organization_id)
    .fetch_all(tx.conn())
    .await?;
    Ok(found
        .into_iter()
        .map(|row| (row.project_id, row.caps))
        .collect())
}

#[derive(sqlx::FromRow)]
struct CapsRow {
    project_id: String,
    #[sqlx(flatten)]
    caps: Caps,
}

/// The project's caps, set, its row made where it has none and filed under
/// the scope's organization; caps of all `None` delete it. Answers what it
/// held before.
pub async fn set_caps<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    caps: &Caps,
) -> sqlx::Result<Caps> {
    let before = caps_of(tx, project_id).await?;
    if *caps == Caps::default() {
        sqlx::query("DELETE FROM telemetry.project_limits WHERE project_id = $1")
            .bind(project_id)
            .execute(tx.conn())
            .await?;
        return Ok(before);
    }
    sqlx::query(
        "INSERT INTO telemetry.project_limits
             (project_id, organization_id, spans, content_bytes, alerts, monitors, objectives,
              alert_deliveries_per_day)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (project_id) DO UPDATE
            SET organization_id = EXCLUDED.organization_id,
                spans = EXCLUDED.spans,
                content_bytes = EXCLUDED.content_bytes,
                alerts = EXCLUDED.alerts,
                monitors = EXCLUDED.monitors,
                objectives = EXCLUDED.objectives,
                alert_deliveries_per_day = EXCLUDED.alert_deliveries_per_day,
                updated_at = now()",
    )
    .bind(project_id)
    .bind(organization_id)
    .bind(caps.spans)
    .bind(caps.content_bytes)
    .bind(caps.alerts)
    .bind(caps.monitors)
    .bind(caps.objectives)
    .bind(caps.alert_deliveries_per_day)
    .execute(tx.conn())
    .await?;
    Ok(before)
}

/// The content the project holds, in bytes: its kept fields and its files.
pub async fn content_held<B: telmoni_shared::db::tenant_session::Binding>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT (SELECT coalesce(sum(bytes), 0) FROM telemetry.plain_content
                  WHERE project_id = $1)
              + (SELECT coalesce(sum(bytes), 0) FROM telemetry.media WHERE project_id = $1)",
    )
    .bind(project_id)
    .fetch_one(tx.conn())
    .await
}

/// Whether the project's own content cap has room: asked only where it has
/// one. A count that cannot be read keeps what came.
pub async fn has_room_for_content(state: &AppState, project_id: &ProjectId) -> bool {
    let Some(cap) = state
        .project_caps
        .of(state, project_id)
        .await
        .content_bytes()
    else {
        return true;
    };
    let read = async {
        let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
        let held = content_held(&mut lane, project_id).await?;
        lane.commit().await?;
        Ok::<_, sqlx::Error>(held)
    };
    match read.await {
        Ok(held) => u64::try_from(held).unwrap_or(0) < cap,
        Err(error) => {
            tracing::warn!(project_id = %project_id, error = %error,
                "telemetry: a project's content held could not be read; what came is kept");
            true
        }
    }
}

/// How long a telling's claim is kept: past the longest period a cap counts.
const TOLD_KEPT_S: i64 = 40 * 24 * 60 * 60;

/// The telling of the project past `what` at `cap` in `period`, claimed in
/// Redis by whichever replica comes first: the claim's key where this one
/// did, none where another had or Redis is out of reach.
pub async fn claim(
    state: &AppState,
    project_id: &ProjectId,
    what: &str,
    cap: u64,
    period: &str,
) -> Option<String> {
    let mut connection = state.redis.connection().await?;
    let told_key = format!("telemetry:project-cap-told:{project_id}:{what}:{period}:{cap}");
    let claimed: Result<(bool,), redis::RedisError> = redis::pipe()
        .atomic()
        .set_nx(&told_key, 1)
        .expire(&told_key, TOLD_KEPT_S)
        .ignore()
        .query_async(&mut connection)
        .await;
    matches!(claimed, Ok((true,))).then_some(told_key)
}

/// The owner and admins told once, for the cap `what` at `cap` in `period`,
/// that the project is past it, naming it: on a task of its own, so the
/// export or the delivery that found it past its cap never waits on the mail.
/// The claim `told_key` is given up where the telling fails, so a later pass
/// tells.
pub async fn tell(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    what: &'static str,
    cap: u64,
    period: String,
    told_key: Option<String>,
) {
    let auth = Arc::clone(&state.auth);
    let notifications = Arc::clone(&state.notifications);
    // Taken now, while the state is to hand: the task outlives the borrow.
    let redis = match told_key {
        Some(_) => state.redis.connection().await,
        None => None,
    };
    let organization_id = organization_id.clone();
    let project_id = project_id.clone();
    tokio::spawn(async move {
        let told = tell_now(
            auth.as_ref(),
            notifications.as_ref(),
            &organization_id,
            &project_id,
            what,
            cap,
            &period,
        )
        .await;
        if !told
            && let Some(told_key) = told_key
            && let Some(mut connection) = redis
        {
            let _: Result<(), redis::RedisError> = connection.del(&told_key).await;
        }
    });
}

/// The telling itself; whether the owners were told.
async fn tell_now(
    auth: &dyn seam::Auth,
    notifications: &dyn seam::Notifications,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    what: &str,
    cap: u64,
    period: &str,
) -> bool {
    if !chain::open(auth, organization_id).await {
        return false;
    }
    let home = auth
        .project_homes(std::slice::from_ref(project_id))
        .await
        .ok()
        .and_then(|homes| homes.into_iter().next());
    let (named, path) = home.map_or_else(
        || ("A project".to_owned(), "/".to_owned()),
        |home| {
            (
                format!("The project {}", home.slug),
                format!("/{}/limits", home.organization_slug),
            )
        },
    );
    let body = format!(
        "{named} is past its cap of {cap} {what}, its share of the organization's. Its other \
         projects carry on. An owner or admin may raise the cap on the organization's Limits \
         page."
    );
    let dedup_key = format!("telemetry:project-cap:{project_id}:{what}:{period}:{cap}");
    let alert = OrganizationAlert {
        title: "A project past its cap",
        body: &body,
        metadata: json!({ "lane": "project_cap", "project_id": project_id, "limit": what,
                          "cap": cap }),
        dedup_key: Some(&dedup_key),
    };
    seam::tell_owners(notifications, auth, organization_id, alert, &path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cap_refuses_at_it_and_none_never_does() {
        let caps = Caps {
            alerts: Some(2),
            monitors: Some(0),
            ..Caps::default()
        };
        assert!(admit(&caps, Counted::Alerts, 1).is_ok());
        let refused = admit(&caps, Counted::Alerts, 2)
            .unwrap_err()
            .to_problem_details();
        assert_eq!(refused.status, 402);
        assert_eq!(
            refused
                .extensions
                .get("entitlement")
                .and_then(|v| v.as_str()),
            Some(PROJECT_LIMIT)
        );
        assert!(refused.detail.unwrap().contains("Limits page"));
        assert!(admit(&caps, Counted::Monitors, 0).is_err());
        assert!(admit(&caps, Counted::Objectives, 1_000).is_ok());
    }
}
