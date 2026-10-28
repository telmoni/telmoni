//! An organization's alert deliveries a UTC day, held to its plan's
//! [`AlertCeilings::deliveries_per_day`] in Redis: a notice takes room for the
//! connectors it would tell and gives back what notifications did not queue.
//! Past the number, incidents reach the feed alone and the owner is told once
//! that day. Without Redis every delivery is made, as a replica's own count
//! would hold the plan once per replica.

use std::sync::LazyLock;

use chrono::{NaiveDate, Utc};
use serde_json::json;

use telmoni_shared::seam::{self, Notice};
use telmoni_shared::{AlertCeilings, NotificationKind, OrganizationId};

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
/// given, and the day and number they were counted against, where they were.
#[derive(Debug, Clone, Copy)]
#[must_use = "the room a notice took is given back by `settle`"]
pub struct Taken {
    wanted: usize,
    room: usize,
    counted: Option<(NaiveDate, u32)>,
}

impl Taken {
    /// The most deliveries the notice may make.
    #[must_use]
    pub const fn room(&self) -> usize {
        self.room
    }
}

/// How many of `wanted` deliveries the organization's day has room for,
/// taken from it until [`settle`] gives back what the notice did not make.
pub async fn take(state: &AppState, organization_id: &OrganizationId, wanted: usize) -> Taken {
    let uncounted = Taken {
        wanted,
        room: wanted,
        counted: None,
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
    let notice = Notice {
        kind: NotificationKind::OrganizationAlert,
        subject_user_id: None,
        title: "Alert deliveries held until midnight UTC",
        body: &body,
        metadata: json!({ "lane": "alert_deliveries", "ceiling": deliveries_per_day }),
        dedup_key: Some(&key),
    };
    if let Err(error) = state
        .notifications
        .emit(organization_id, None, notice)
        .await
    {
        tracing::warn!(
            organization_id = %organization_id,
            error = %error,
            "telemetry: the owner could not be told the day's alert deliveries are spent"
        );
    }
}
