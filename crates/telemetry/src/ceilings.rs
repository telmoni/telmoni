//! The machine lanes' ceiling each organization's plan sets: data out a UTC
//! day, counted where the read API, the CLI's lanes and the exports hand bytes
//! to an organization, the CLI's never refused. The number is the deployment's,
//! read through the entitlements seam and kept a minute per organization; a
//! binary that sells nothing answers none, and nothing is held or counted.
//! Each refusal is counted for the organization's owner and admins, told in
//! the feed once a day while it lasts ([`run`]). Counts live in Redis, shared
//! by every replica, and in this process while it does not answer, as the
//! rate limits do.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, Utc};
use redis::AsyncCommands;
use serde_json::json;

use telmoni_shared::seam::{self, Entitlements, Notice};
use telmoni_shared::{LaneCeilings, NotificationKind, OrganizationId, TelmoniError, TenantError};

use crate::AppState;
use crate::handler::chain;

/// How long an organization's ceilings are kept: a plan bought or left is
/// held to within a minute.
const KEEP_FOR: Duration = Duration::from_secs(60);

/// How many organizations' answers are kept at once.
const KNOWN_MAX: usize = 10_000;

/// How long a day's counts outlive it, so one made just before midnight is
/// still told after it.
const COUNTS_KEPT_S: i64 = 2 * 24 * 60 * 60;

/// How often the owners' notices are looked for.
const TELL_EVERY: Duration = Duration::from_secs(300);

/// The loop's name in `telemetry.heartbeats`.
pub const HEARTBEAT: &str = "refusals";

/// The lane a refusal is counted and told under.
const LANE: &str = "data_out";

/// A day's refusals: how many, and since when.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Refused {
    count: u64,
    since: DateTime<Utc>,
}

/// What this process holds while Redis does not answer.
#[derive(Default)]
struct Local {
    data_out: HashMap<(OrganizationId, NaiveDate), u64>,
    days: HashMap<(OrganizationId, NaiveDate), Refused>,
}

/// The ceilings as one replica holds them.
pub struct Ceilings {
    known: Mutex<HashMap<OrganizationId, (Option<LaneCeilings>, Instant)>>,
    local: Mutex<Local>,
    /// The notices this replica has had written, so each is asked once.
    told: Mutex<HashSet<(OrganizationId, NaiveDate)>>,
}

impl Default for Ceilings {
    fn default() -> Self {
        Self {
            known: Mutex::new(HashMap::new()),
            local: Mutex::new(Local::default()),
            told: Mutex::new(HashSet::new()),
        }
    }
}

impl Ceilings {
    /// The organization's ceilings, from memory for a minute after the seam
    /// last answered.
    pub async fn of(
        &self,
        entitlements: &dyn Entitlements,
        organization_id: &OrganizationId,
    ) -> Option<LaneCeilings> {
        if let Some((ceilings, at)) = self
            .known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(organization_id)
            && at.elapsed() < KEEP_FOR
        {
            return ceilings.clone();
        }
        let ceilings = seam::lane_ceilings(entitlements, organization_id).await;
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        if known.len() >= KNOWN_MAX {
            known.retain(|_, (_, at)| at.elapsed() < KEEP_FOR);
        }
        known.insert(organization_id.clone(), (ceilings.clone(), Instant::now()));
        ceilings
    }

    fn with_local<T>(&self, f: impl FnOnce(&mut Local) -> T) -> T {
        f(&mut self.local.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Whether the organization's machines may be sent more today: refused, a
/// 429 until midnight UTC, once its plan's day is spent.
pub async fn admit_data_out(
    state: &AppState,
    organization_id: &OrganizationId,
) -> Result<(), TelmoniError> {
    let Some(plan) = state
        .ceilings
        .of(state.entitlements.as_ref(), organization_id)
        .await
    else {
        return Ok(());
    };
    let now = Utc::now();
    let day = now.date_naive();
    if spent(state, organization_id, day).await < plan.data_out_per_day {
        return Ok(());
    }
    refused(state, organization_id).await;
    Err(TenantError::PastCeiling {
        detail: format!(
            "this organization has been sent the {} of data out its plan holds a day, so the \
             read API and exports resume at midnight UTC",
            megabytes(plan.data_out_per_day)
        ),
        retry_after_secs: until_midnight(now),
    }
    .into())
}

/// `bytes` handed to the organization, counted against its day; nothing is
/// counted for an organization no plan holds to a ceiling.
pub async fn spend_data_out(state: &AppState, organization_id: &OrganizationId, bytes: u64) {
    if state
        .ceilings
        .of(state.entitlements.as_ref(), organization_id)
        .await
        .is_none()
    {
        return;
    }
    let day = Utc::now().date_naive();
    let key = data_out_key(organization_id, day);
    if let Some(mut connection) = state.redis.connection().await {
        let added: Result<(), redis::RedisError> = redis::pipe()
            .atomic()
            .incr(&key, bytes)
            .ignore()
            .expire(&key, COUNTS_KEPT_S)
            .ignore()
            .query_async(&mut connection)
            .await;
        match added {
            Ok(()) => return,
            Err(error) => state.redis.stood_in("the day's data out", &error),
        }
    }
    state.ceilings.with_local(|local| {
        *local
            .data_out
            .entry((organization_id.clone(), day))
            .or_default() += bytes;
        local
            .data_out
            .retain(|(_, d), _| *d >= day.pred_opt().unwrap_or(day));
    });
}

/// What the organization has been sent today.
async fn spent(state: &AppState, organization_id: &OrganizationId, day: NaiveDate) -> u64 {
    if let Some(mut connection) = state.redis.connection().await {
        let read: Result<Option<u64>, redis::RedisError> =
            connection.get(data_out_key(organization_id, day)).await;
        match read {
            Ok(spent) => return spent.unwrap_or(0),
            Err(error) => state.redis.stood_in("the day's data out", &error),
        }
    }
    state.ceilings.with_local(|local| {
        local
            .data_out
            .get(&(organization_id.clone(), day))
            .copied()
            .unwrap_or(0)
    })
}

fn data_out_key(organization_id: &OrganizationId, day: NaiveDate) -> String {
    format!("telemetry:data-out:{organization_id}:{day}")
}

/// Seconds until the next midnight UTC, rounded up so a client that waits
/// them is not refused again, at least one.
fn until_midnight(now: DateTime<Utc>) -> u64 {
    let midnight = now
        .date_naive()
        .succ_opt()
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .map(|at| at.and_utc());
    midnight.map_or(1, |at| {
        u64::try_from((at - now).num_milliseconds())
            .unwrap_or(1)
            .div_ceil(1_000)
            .max(1)
    })
}

/// A ceiling of bytes as a refusal names it: 5 MB, 2.5 MB.
fn megabytes(bytes: u64) -> String {
    let whole = bytes / 1_000_000;
    if bytes.is_multiple_of(1_000_000) {
        format!("{whole} MB")
    } else {
        format!("{whole}.{} MB", bytes % 1_000_000 / 100_000)
    }
}

/// A refusal counted for the owner's notice of the day.
async fn refused(state: &AppState, organization_id: &OrganizationId) {
    let now = Utc::now();
    let day = now.date_naive();
    if let Some(mut connection) = state.redis.connection().await {
        let (index, counts) = refused_keys(organization_id, day);
        let counted: Result<(), redis::RedisError> = redis::pipe()
            .atomic()
            .hincr(&counts, LANE, 1)
            .ignore()
            .hset_nx(&counts, format!("{LANE}:since"), now.to_rfc3339())
            .ignore()
            .expire(&counts, COUNTS_KEPT_S)
            .ignore()
            .sadd(&index, organization_id.as_str())
            .ignore()
            .expire(&index, COUNTS_KEPT_S)
            .ignore()
            .query_async(&mut connection)
            .await;
        match counted {
            Ok(()) => return,
            Err(error) => state.redis.stood_in("a refusal's count", &error),
        }
    }
    state.ceilings.with_local(|local| {
        local
            .days
            .entry((organization_id.clone(), day))
            .or_insert(Refused {
                count: 0,
                since: now,
            })
            .count += 1;
        local
            .days
            .retain(|(_, d), _| *d >= day.pred_opt().unwrap_or(day));
    });
}

/// The day's index of organizations refused, and each one's counts.
fn refused_keys(organization_id: &OrganizationId, day: NaiveDate) -> (String, String) {
    (
        format!("telemetry:refused:{day}"),
        format!("telemetry:refused:{day}:{organization_id}"),
    )
}

/// Every organization refused on `day`, as Redis or this process holds it.
async fn days(state: &AppState, day: NaiveDate) -> Vec<(OrganizationId, Refused)> {
    if let Some(mut connection) = state.redis.connection().await {
        match read_days(&mut connection, day).await {
            Ok(days) => return days,
            Err(error) => state.redis.stood_in("the day's refusals", &error),
        }
    }
    state.ceilings.with_local(|local| {
        local
            .days
            .iter()
            .filter(|((_, d), _)| *d == day)
            .map(|((organization_id, _), refused)| (organization_id.clone(), refused.clone()))
            .collect()
    })
}

async fn read_days(
    connection: &mut redis::aio::ConnectionManager,
    day: NaiveDate,
) -> Result<Vec<(OrganizationId, Refused)>, redis::RedisError> {
    let index: Vec<String> = connection
        .smembers(format!("telemetry:refused:{day}"))
        .await?;
    let mut days = Vec::with_capacity(index.len());
    for organization in index {
        let Ok(organization_id) = OrganizationId::try_new(organization.as_str()) else {
            continue;
        };
        let (_, counts) = refused_keys(&organization_id, day);
        let fields: HashMap<String, String> = connection.hgetall(&counts).await?;
        let Some(count) = fields.get(LANE).and_then(|c| c.parse().ok()) else {
            continue;
        };
        let since = fields
            .get(&format!("{LANE}:since"))
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
            .map_or_else(Utc::now, |at| at.with_timezone(&Utc));
        days.push((organization_id, Refused { count, since }));
    }
    Ok(days)
}

/// The notice an organization's day of refusals is told as.
fn notice_text(refused: &Refused) -> (String, String) {
    let since = refused.since.format("%H:%M UTC on %-d %B");
    let often = how_often(refused.count);
    (
        "Data out refused past the day's ceiling".to_owned(),
        format!(
            "This organization's machines have been sent the data out its plan holds for the \
             day: the read API and exports have been refused {often} since {since}, and are \
             let through again at midnight UTC."
        ),
    )
}

/// How often a notice says something happened: "once", "3 times".
fn how_often(count: u64) -> String {
    if count == 1 {
        "once".to_owned()
    } else {
        format!("{count} times")
    }
}

/// Tell each organization refused today, and yesterday: one notice a day,
/// written once whichever replica asks (its dedup key), and asked once by
/// each replica.
pub async fn tell(state: &AppState) {
    let today = Utc::now().date_naive();
    for day in [today.pred_opt().unwrap_or(today), today] {
        for (organization_id, refused) in days(state, day).await {
            let told = (organization_id.clone(), day);
            if state
                .ceilings
                .told
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&told)
            {
                continue;
            }
            // An organization erased or being erased is told nothing, its
            // feed purged or about to be.
            if !chain::open(state.auth.as_ref(), &organization_id).await {
                state
                    .ceilings
                    .told
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(told);
                continue;
            }
            let (title, body) = notice_text(&refused);
            let dedup_key = format!("telemetry:refused:{organization_id}:{LANE}:{day}");
            let notice = Notice {
                kind: NotificationKind::OrganizationAlert,
                subject_user_id: None,
                title: &title,
                body: &body,
                metadata: json!({
                    "lane": LANE,
                    "refused": refused.count,
                    "since": refused.since,
                    "day": day,
                }),
                dedup_key: Some(&dedup_key),
            };
            match state
                .notifications
                .emit(&organization_id, None, notice)
                .await
            {
                Ok(_) => {
                    let mut told_set = state
                        .ceilings
                        .told
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    told_set.retain(|(_, d)| *d >= today.pred_opt().unwrap_or(today));
                    told_set.insert(told);
                }
                Err(error) => tracing::warn!(
                    organization_id = %organization_id,
                    lane = LANE,
                    error = %error,
                    "telemetry: a day's refusals could not be told; the next pass tries again"
                ),
            }
        }
    }
}

/// [`tell`], every five minutes, forever.
pub async fn run(state: Arc<AppState>) {
    state
        .heartbeats
        .every(&state.db, HEARTBEAT, Duration::ZERO, TELL_EVERY, || {
            let state = state.clone();
            async move {
                tell(&state).await;
                Ok::<(), std::convert::Infallible>(())
            }
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ceiling_reads_in_round_megabytes_and_midnight_is_never_now() {
        assert_eq!(megabytes(5_000_000), "5 MB");
        assert_eq!(megabytes(125_000_000), "125 MB");
        assert_eq!(megabytes(2_500_000), "2.5 MB");
        let late = DateTime::parse_from_rfc3339("2026-10-22T23:59:59.5Z")
            .expect("a time")
            .with_timezone(&Utc);
        assert_eq!(until_midnight(late), 1);
        let noon = DateTime::parse_from_rfc3339("2026-10-22T12:00:00Z")
            .expect("a time")
            .with_timezone(&Utc);
        assert_eq!(until_midnight(noon), 12 * 3_600);
    }

    /// A notice names the count and the time it began, and never a person.
    #[test]
    fn a_days_notice_says_how_many_and_since_when() {
        let refused = Refused {
            count: 14,
            since: DateTime::parse_from_rfc3339("2026-10-22T14:02:00Z")
                .expect("a time")
                .with_timezone(&Utc),
        };
        let (_, body) = notice_text(&refused);
        assert!(
            body.contains("refused 14 times")
                && body.contains("14:02 UTC on 22 October")
                && body.contains("midnight UTC"),
            "{body}"
        );
        let once = Refused {
            count: 1,
            ..refused
        };
        let (_, body) = notice_text(&once);
        assert!(body.contains("refused once"), "{body}");
    }
}
