//! An organization's alert deliveries a UTC day, held to its plan's second
//! number ([`AlertCeilings::deliveries_per_day`]): counted in Redis per
//! organization and day, as the connectors each notice goes to, the way data
//! out is. A delivery past it is not made, while its incident stands in the
//! feed, and the owner and admins are told once that day. With no ceiling, or
//! without Redis, every delivery is made: a replica's own count would hold
//! the plan once per replica.

use chrono::{NaiveDate, Utc};
use serde_json::json;

use telmoni_shared::seam::{self, Notice};
use telmoni_shared::{AlertCeilings, NotificationKind, OrganizationId};

use crate::AppState;

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

/// How long a day's count outlives it, so one made just before midnight is
/// still read after it.
const KEPT_S: i64 = 2 * 24 * 60 * 60;

/// How many of `wanted` deliveries the organization's day has room for,
/// taken from it; where it had room for fewer, the owner and admins are told.
pub async fn take(state: &AppState, organization_id: &OrganizationId, wanted: usize) -> usize {
    if wanted == 0 {
        return 0;
    }
    let Some(AlertCeilings {
        deliveries_per_day, ..
    }) = seam::alert_ceilings(state.entitlements.as_ref(), organization_id).await
    else {
        return wanted;
    };
    let day = Utc::now().date_naive();
    let Some(mut connection) = state.redis.connection().await else {
        return wanted;
    };
    let taken: Result<i64, redis::RedisError> = redis::Script::new(TAKE)
        .key(day_key(organization_id, day))
        .arg(deliveries_per_day)
        .arg(wanted)
        .arg(KEPT_S)
        .invoke_async(&mut connection)
        .await;
    let taken = match taken {
        Ok(taken) => usize::try_from(taken).unwrap_or(0),
        Err(error) => {
            state.redis.stood_in("the day's alert deliveries", &error);
            return wanted;
        }
    };
    if taken < wanted {
        tell_owner(state, organization_id, deliveries_per_day, day).await;
    }
    taken
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
    let key = format!("telemetry:alert-deliveries:{organization_id}:{day}");
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
