//! An organization's alert deliveries a UTC day, held to its plan's
//! [`AlertCeilings::deliveries_per_day`] in Redis: a notice takes room for the
//! connectors it would tell and gives back what notifications did not queue.
//! Past the number, incidents reach the feed alone and the owner is told once
//! that day. A project its owners capped (`crate::project_limits`) is held to
//! its own share first, the same way. Without Redis every delivery is made, as
//! a replica's own count would hold the plan once per replica.

use std::sync::LazyLock;

use chrono::{NaiveDate, Utc};
use serde_json::json;

use telmoni_shared::seam::{self, OrganizationAlert};
use telmoni_shared::{AlertCeilings, OrganizationId, ProjectId};

use crate::AppState;

/// The scripts, hashed once rather than at every notice.
static TAKE_SCRIPT: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(TAKE));
static GIVE_BACK_SCRIPT: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(GIVE_BACK));

/// Up to `ARGV[2]` deliveries taken from the day's room under the ceiling
/// `ARGV[1]`, the key kept `ARGV[3]` seconds: answers how many were taken.
const TAKE: &str = r"
local spent = tonumber(redis.call('GET', KEYS[1]) or '0')
local room = math.max(0, tonumber(ARGV[1]) - spent)
local take = math.min(room, tonumber(ARGV[2]))
if take > 0 then
  redis.call('INCRBY', KEYS[1], take)
end
redis.call('EXPIRE', KEYS[1], ARGV[3])
return take
";

/// `ARGV[1]` deliveries given back to the day's room, never below none, nor
/// to a day whose count is gone.
const GIVE_BACK: &str = r"
if redis.call('EXISTS', KEYS[1]) == 0 then
  return 0
end
local left = redis.call('DECRBY', KEYS[1], ARGV[1])
if left < 0 then
  redis.call('SET', KEYS[1], 0, 'KEEPTTL')
end
return left
";

/// How long a day's count outlives it, so one made just before midnight is
/// still read after it.
const KEPT_S: i64 = 2 * 24 * 60 * 60;

/// The room a notice took for its deliveries: how many it wanted and was
/// given, the day and number they were counted against, where they were,
/// and its project's own share, where its owners capped one.
#[derive(Debug, Clone)]
#[must_use = "the room a notice took is given back by `settle`"]
pub struct Taken {
    wanted: usize,
    room: usize,
    counted: Option<(NaiveDate, u32)>,
    project: Option<ProjectTaken>,
}

/// What a notice took of its project's own share of the day.
#[derive(Debug, Clone)]
struct ProjectTaken {
    project_id: ProjectId,
    day: NaiveDate,
    cap: u64,
    room: usize,
}

impl Taken {
    /// The most deliveries the notice may make.
    #[must_use]
    pub const fn room(&self) -> usize {
        self.room
    }
}

/// How many of `wanted` deliveries the project's own share, where capped,
/// and then the organization's day have room for, taken from each until
/// [`settle`] gives back what the notice did not make.
pub async fn take(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    wanted: usize,
) -> Taken {
    let project = take_project(state, project_id, wanted).await;
    let asked = project.as_ref().map_or(wanted, |project| project.room);
    let mut taken = take_organization(state, organization_id, asked).await;
    taken.wanted = wanted;
    taken.project = project;
    taken
}

/// Up to `wanted` deliveries from the project's own share of the day, where
/// its owners capped one; none taken where Redis is out of reach.
async fn take_project(
    state: &AppState,
    project_id: &ProjectId,
    wanted: usize,
) -> Option<ProjectTaken> {
    if wanted == 0 {
        return None;
    }
    let cap = state
        .project_caps
        .of(state, project_id)
        .await
        .alert_deliveries_per_day()?;
    let day = Utc::now().date_naive();
    let mut connection = state.redis.connection().await?;
    let taken: Result<i64, redis::RedisError> = TAKE_SCRIPT
        .key(project_day_key(project_id, day))
        .arg(cap)
        .arg(wanted)
        .arg(KEPT_S)
        .invoke_async(&mut connection)
        .await;
    match taken {
        Ok(taken) => Some(ProjectTaken {
            project_id: project_id.clone(),
            day,
            cap,
            room: usize::try_from(taken).unwrap_or(0),
        }),
        Err(error) => {
            state.redis.stood_in("a project's alert deliveries", &error);
            None
        }
    }
}

/// How many of `wanted` deliveries the organization's day has room for.
async fn take_organization(
    state: &AppState,
    organization_id: &OrganizationId,
    wanted: usize,
) -> Taken {
    let uncounted = Taken {
        wanted,
        room: wanted,
        counted: None,
        project: None,
    };
    if wanted == 0 {
        return uncounted;
    }
    let Some(AlertCeilings {
        deliveries_per_day, ..
    }) = seam::alert_ceilings(state.entitlements.as_ref(), organization_id).await
    else {
        return uncounted;
    };
    let day = Utc::now().date_naive();
    let Some(mut connection) = state.redis.connection().await else {
        return uncounted;
    };
    let taken: Result<i64, redis::RedisError> = TAKE_SCRIPT
        .key(day_key(organization_id, day))
        .arg(deliveries_per_day)
        .arg(wanted)
        .arg(KEPT_S)
        .invoke_async(&mut connection)
        .await;
    match taken {
        Ok(taken) => Taken {
            wanted,
            room: usize::try_from(taken).unwrap_or(0),
            counted: Some((day, deliveries_per_day)),
            project: None,
        },
        Err(error) => {
            state.redis.stood_in("the day's alert deliveries", &error);
            uncounted
        }
    }
}

/// A notice's deliveries settled against its day: the room it took and did
/// not use given back, its connections having heard fewer than it took; and
/// where the day's room ran out before every connection that would hear it,
/// the owner and admins told.
pub async fn settle(state: &AppState, organization_id: &OrganizationId, taken: Taken, made: usize) {
    if let Some(project) = &taken.project {
        settle_project(state, organization_id, project, taken.wanted, made).await;
    }
    let Some((day, deliveries_per_day)) = taken.counted else {
        return;
    };
    let unused = taken.room.saturating_sub(made);
    if unused > 0 {
        give_back(state, organization_id, day, unused).await;
    } else if taken.room < taken.wanted {
        tell_owner(state, organization_id, deliveries_per_day, day).await;
    }
}

/// `unused` deliveries given back to the organization's room on `day`.
async fn give_back(
    state: &AppState,
    organization_id: &OrganizationId,
    day: NaiveDate,
    unused: usize,
) {
    let Some(mut connection) = state.redis.connection().await else {
        return;
    };
    let given: Result<i64, redis::RedisError> = GIVE_BACK_SCRIPT
        .key(day_key(organization_id, day))
        .arg(unused)
        .invoke_async(&mut connection)
        .await;
    if let Err(error) = given {
        state.redis.stood_in("the day's alert deliveries", &error);
    }
}

/// The project's share given back what the notice did not make of it, and
/// where its share ran out before every connection that would hear it, the
/// owner and admins told once that day.
async fn settle_project(
    state: &AppState,
    organization_id: &OrganizationId,
    project: &ProjectTaken,
    wanted: usize,
    made: usize,
) {
    let unused = project.room.saturating_sub(made);
    if unused > 0 {
        if let Some(mut connection) = state.redis.connection().await {
            let given: Result<i64, redis::RedisError> = GIVE_BACK_SCRIPT
                .key(project_day_key(&project.project_id, project.day))
                .arg(unused)
                .invoke_async(&mut connection)
                .await;
            if let Err(error) = given {
                state.redis.stood_in("a project's alert deliveries", &error);
            }
        }
    } else if project.room < wanted {
        let what = "alert deliveries a day";
        let day = project.day.to_string();
        if let Some(told_key) =
            crate::project_limits::claim(state, &project.project_id, what, project.cap, &day).await
        {
            crate::project_limits::tell_now(
                state,
                organization_id,
                &project.project_id,
                what,
                project.cap,
                &day,
                Some(told_key),
            )
            .await;
        }
    }
}

/// A project's own count of alert deliveries on `day`.
fn project_day_key(project_id: &ProjectId, day: NaiveDate) -> String {
    format!("telemetry:project-alert-deliveries:{project_id}:{day}")
}

/// The organization's count of alert deliveries on `day`.
fn day_key(organization_id: &OrganizationId, day: NaiveDate) -> String {
    format!("telemetry:alert-deliveries:{organization_id}:{day}")
}

/// The owner and admins told, once a day by the feed's own key, that the
/// organization's alerts reach the feed alone until midnight UTC.
async fn tell_owner(
    state: &AppState,
    organization_id: &OrganizationId,
    deliveries_per_day: u32,
    day: NaiveDate,
) {
    let key = day_key(organization_id, day);
    let body = format!(
        "This organization's alerts have made the {deliveries_per_day} deliveries its plan holds \
         a UTC day, so until midnight UTC their incidents reach the feed alone."
    );
    let alert = OrganizationAlert {
        title: "Alert deliveries held until midnight UTC",
        body: &body,
        metadata: json!({ "lane": "alert_deliveries", "ceiling": deliveries_per_day }),
        dedup_key: Some(&key),
    };
    if let Err(error) = state
        .notifications
        .alert_organization(organization_id, alert)
        .await
    {
        tracing::warn!(
            organization_id = %organization_id,
            error = %error,
            "telemetry: the owner could not be told the day's alert deliveries are spent"
        );
    }
}
