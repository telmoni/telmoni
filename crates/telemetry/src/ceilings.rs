//! The machine lanes' ceilings each organization's plan sets: pings a minute,
//! held in the ping lane per organization, and data out a UTC day, counted
//! where the read API, the CLI's lanes and the exports hand bytes to an
//! organization, the CLI's never refused. The numbers are the deployment's, read through
//! the entitlements seam and kept a minute per organization; a binary that
//! sells nothing answers none, and nothing is held or counted. Each refusal
//! is counted for the organization's owner and admins, told in the feed once
//! a day for each lane while it lasts ([`run`]). Counts live in Redis, shared
//! by every replica, and in this process while it does not answer, as the
//! rate limits do.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, Utc};
use redis::AsyncCommands;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped};
use telmoni_shared::seam::{self, Entitlements, Notice};
use telmoni_shared::types::MICRO_PINGS;
use telmoni_shared::{LaneCeilings, NotificationKind, OrganizationId, TelmoniError, TenantError};

use crate::AppState;
use crate::db::{self, ResolvedPingKey};
use crate::handler::chain;
use crate::rate_limit::RateLimit;
use crate::schedule::Schedule;

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

/// The most slugs a day's notice names, and so the most kept.
const SLUGS_MAX: usize = 10;

/// A slug added to the day's set while it holds fewer than `ARGV[1]`, the
/// set kept `ARGV[3]` seconds: an organization naming new slugs all day
/// grows nothing past what its notice reads.
const ADD_SLUG: &str = r"
if redis.call('SCARD', KEYS[1]) < tonumber(ARGV[1]) then
    redis.call('SADD', KEYS[1], ARGV[2])
end
redis.call('EXPIRE', KEYS[1], ARGV[3])
return 1
";

/// A lane an organization can be refused on for its plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lane {
    /// A ping past the plan's pings a minute.
    Pings,
    /// An answer or an export past the day's data out.
    DataOut,
    /// A ping or a run naming a monitor the organization has none of, none
    /// made for the plan's monitors.
    Monitors,
}

impl Lane {
    const ALL: [Self; 3] = [Self::Pings, Self::DataOut, Self::Monitors];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Pings => "pings",
            Self::DataOut => "data_out",
            Self::Monitors => "monitors",
        }
    }
}

/// A lane's refusals in a day: how many, and since when.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Refused {
    count: u64,
    since: DateTime<Utc>,
}

/// One organization's day, as the notices read it.
#[derive(Debug, Default, Clone)]
struct Day {
    refused: HashMap<Lane, Refused>,
    slugs: BTreeSet<String>,
}

/// What this process holds while Redis does not answer.
#[derive(Default)]
struct Local {
    data_out: HashMap<(OrganizationId, NaiveDate), u64>,
    days: HashMap<(OrganizationId, NaiveDate), Day>,
}

/// The ceilings as one replica holds them.
pub struct Ceilings {
    known: Mutex<HashMap<OrganizationId, (Option<LaneCeilings>, Instant)>>,
    ping_organization: RateLimit,
    add_slug: redis::Script,
    local: Mutex<Local>,
    /// The notices this replica has had written, so each is asked once.
    told: Mutex<HashSet<(OrganizationId, Lane, NaiveDate)>>,
}

impl Default for Ceilings {
    fn default() -> Self {
        Self {
            known: Mutex::new(HashMap::new()),
            // The rate it is held to is the plan's, given at each ping.
            ping_organization: RateLimit::new("telemetry:ping:organization", 1),
            add_slug: redis::Script::new(ADD_SLUG),
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

/// A ping held to its organization's plan, across its projects and keys:
/// refused with how long to wait, unrecorded, past it. The door holds each
/// key to its own limit already.
pub async fn admit_ping(state: &AppState, key: &ResolvedPingKey) -> Result<(), TelmoniError> {
    let ceilings = &state.ceilings;
    let Some(plan) = ceilings
        .of(state.entitlements.as_ref(), &key.organization_id)
        .await
    else {
        return Ok(());
    };
    let (rate, burst) = (plan.pings_per_minute, plan.ping_burst());
    let Some(wait_ms) = ceilings
        .ping_organization
        .wait_at(&state.redis, key.organization_id.as_str(), rate, burst)
        .await
    else {
        return Ok(());
    };
    refused(state, &key.organization_id, Lane::Pings, None).await;
    tracing::debug!(
        organization_id = %key.organization_id,
        ping_key_id = %key.id,
        "ping: refused past the plan's pings a minute"
    );
    let retry_after_secs = wait_ms.div_ceil(1_000).max(1);
    Err(TenantError::PastCeiling {
        detail: format!(
            "this organization's monitors pinged past the {rate} a minute its plan holds across \
             its projects, so this ping was not recorded; retry after {retry_after_secs}s"
        ),
        retry_after_secs,
    }
    .into())
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
    refused(state, organization_id, Lane::DataOut, None).await;
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

/// A ceiling of bytes as `PLANS.md` writes it: 5 MB, 2.5 MB.
fn megabytes(bytes: u64) -> String {
    let whole = bytes / 1_000_000;
    if bytes.is_multiple_of(1_000_000) {
        format!("{whole} MB")
    } else {
        format!("{whole}.{} MB", bytes % 1_000_000 / 100_000)
    }
}

/// Pings a minute, in millionths, that a schedule whose runs come at least
/// `gap_s` apart needs: two a run, its start and its end.
#[must_use]
pub fn need_of_gap(gap_s: i64) -> u64 {
    (120 * MICRO_PINGS).div_ceil(u64::try_from(gap_s).unwrap_or(1).max(1))
}

/// What a timing needs, from now: nothing for one that expects no run, or
/// one that cannot be read now (the monitors lane logs that when it is due).
#[must_use]
pub fn need_of(schedule: &Schedule, now: DateTime<Utc>) -> u64 {
    match schedule.shortest_gap_s(now) {
        Ok(Some(gap)) => need_of_gap(gap),
        Ok(None) | Err(_) => 0,
    }
}

/// Whether the organization's schedules, `changing` one of them set to
/// `timing`, fit the pings its plan holds: each counted by its tightest pair
/// of runs, two pings a run, paused or not, since a paused monitor's crontab
/// still pings. Read under the organization's chain lock, so two saves at
/// once are counted one after the other.
pub async fn admit_schedules(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    organization_id: &OrganizationId,
    plan: Option<&LaneCeilings>,
    changing: Option<Uuid>,
    timing: &Schedule,
) -> Result<(), TelmoniError> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let now = Utc::now();
    // Many monitors share a line, which is searched once.
    let mut known: HashMap<(Option<String>, Option<i32>, String), u64> = HashMap::new();
    let (mut before, mut others) = (0_u64, 0_u64);
    for row in db::organization_timings(tx, organization_id).await? {
        let id = row.id;
        let need = *known
            .entry((row.schedule, row.interval_s, row.timezone))
            .or_insert_with_key(|(schedule, interval_s, timezone)| {
                Schedule::read(schedule.as_deref(), *interval_s, timezone)
                    .map_or(0, |schedule| need_of(&schedule, now))
            });
        before = before.saturating_add(need);
        if Some(id) != changing {
            others = others.saturating_add(need);
        }
    }
    plan.admit_schedules(before, others.saturating_add(need_of(timing, now)))
}

/// A refusal counted for the owner's notice of the day: `slug` the monitor
/// a ping named, where the plan's monitors refused one.
pub async fn refused(
    state: &AppState,
    organization_id: &OrganizationId,
    lane: Lane,
    slug: Option<&str>,
) {
    let now = Utc::now();
    let day = now.date_naive();
    if let Some(mut connection) = state.redis.connection().await {
        let (index, counts, slugs) = refused_keys(organization_id, day);
        let mut pipe = redis::pipe();
        pipe.atomic()
            .hincr(&counts, lane.as_str(), 1)
            .ignore()
            .hset_nx(
                &counts,
                format!("{}:since", lane.as_str()),
                now.to_rfc3339(),
            )
            .ignore()
            .expire(&counts, COUNTS_KEPT_S)
            .ignore()
            .sadd(&index, organization_id.as_str())
            .ignore()
            .expire(&index, COUNTS_KEPT_S)
            .ignore();
        let mut counted: Result<(), redis::RedisError> = pipe.query_async(&mut connection).await;
        if let (Ok(()), Some(slug)) = (&counted, slug) {
            counted = state
                .ceilings
                .add_slug
                .key(&slugs)
                .arg(SLUGS_MAX)
                .arg(slug)
                .arg(COUNTS_KEPT_S)
                .invoke_async::<i64>(&mut connection)
                .await
                .map(drop);
        }
        match counted {
            Ok(()) => return,
            Err(error) => state.redis.stood_in("a refusal's count", &error),
        }
    }
    state.ceilings.with_local(|local| {
        let entry = local
            .days
            .entry((organization_id.clone(), day))
            .or_default();
        let refused = entry.refused.entry(lane).or_insert(Refused {
            count: 0,
            since: now,
        });
        refused.count += 1;
        if let Some(slug) = slug
            && entry.slugs.len() < SLUGS_MAX
        {
            entry.slugs.insert(slug.to_owned());
        }
        local
            .days
            .retain(|(_, d), _| *d >= day.pred_opt().unwrap_or(day));
    });
}

/// The day's index of organizations refused, each one's counts, and the
/// slugs its pings and runs named.
fn refused_keys(organization_id: &OrganizationId, day: NaiveDate) -> (String, String, String) {
    (
        format!("telemetry:refused:{day}"),
        format!("telemetry:refused:{day}:{organization_id}"),
        format!("telemetry:refused:{day}:{organization_id}:slugs"),
    )
}

/// Every organization refused on `day`, as Redis or this process holds it.
async fn days(state: &AppState, day: NaiveDate) -> Vec<(OrganizationId, Day)> {
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
) -> Result<Vec<(OrganizationId, Day)>, redis::RedisError> {
    let index: Vec<String> = connection
        .smembers(format!("telemetry:refused:{day}"))
        .await?;
    let mut days = Vec::with_capacity(index.len());
    for organization in index {
        let Ok(organization_id) = OrganizationId::try_new(organization.as_str()) else {
            continue;
        };
        let (_, counts, slugs) = refused_keys(&organization_id, day);
        let fields: HashMap<String, String> = connection.hgetall(&counts).await?;
        let mut refused = Day::default();
        for lane in Lane::ALL {
            let Some(count) = fields.get(lane.as_str()).and_then(|c| c.parse().ok()) else {
                continue;
            };
            let since = fields
                .get(&format!("{}:since", lane.as_str()))
                .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                .map_or_else(Utc::now, |at| at.with_timezone(&Utc));
            refused.refused.insert(lane, Refused { count, since });
        }
        let named: Vec<String> = connection
            .srandmember_multiple(&slugs, isize::try_from(SLUGS_MAX).unwrap_or(isize::MAX))
            .await?;
        refused.slugs = named.into_iter().collect();
        days.push((organization_id, refused));
    }
    Ok(days)
}

/// The notice an organization's day of refusals on one lane is told as.
fn notice_text(lane: Lane, refused: &Refused, slugs: &BTreeSet<String>) -> (String, String) {
    let since = refused.since.format("%H:%M UTC on %-d %B");
    let often = how_often(refused.count);
    match lane {
        Lane::Pings => (
            "Pings refused past the plan's ceiling".to_owned(),
            format!(
                "This organization's monitors pinged more often than its plan holds across its \
                 projects: pings have been refused {often} since {since} and were not recorded, \
                 so their monitors may look late. The usual cause is a crontab line running more \
                 often than its monitor's schedule says."
            ),
        ),
        Lane::DataOut => (
            "Data out refused past the day's ceiling".to_owned(),
            format!(
                "This organization's machines have been sent the data out its plan holds for the \
                 day: the read API and exports have been refused {often} since {since}, and are \
                 let through again at midnight UTC."
            ),
        ),
        Lane::Monitors => {
            let named: Vec<&str> = slugs.iter().map(String::as_str).collect();
            let named = if named.is_empty() {
                String::new()
            } else {
                format!(" They named {}.", named.join(", "))
            };
            (
                "Monitors not made past the plan's ceiling".to_owned(),
                format!(
                    "Pings and runs have named a monitor this organization does not have \
                     {often} since {since}, and none was made, since it holds every monitor its \
                     plan does: the pings were not recorded, and the runs' spans were kept with \
                     no monitor watching them.{named} Delete a monitor, or raise the plan's \
                     ceiling."
                ),
            )
        }
    }
}

/// How often a notice says something happened: "once", "3 times".
fn how_often(count: u64) -> String {
    if count == 1 {
        "once".to_owned()
    } else {
        format!("{count} times")
    }
}

/// Tell each organization refused today, and yesterday, of each lane it was
/// refused on: one notice a lane a day, written once whichever replica asks
/// (its dedup key), and asked once by each replica.
pub async fn tell(state: &AppState) {
    let today = Utc::now().date_naive();
    for day in [today.pred_opt().unwrap_or(today), today] {
        for (organization_id, refused) in days(state, day).await {
            for (lane, counted) in &refused.refused {
                let told = (organization_id.clone(), *lane, day);
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
                let (title, body) = notice_text(*lane, counted, &refused.slugs);
                let dedup_key = format!(
                    "telemetry:refused:{organization_id}:{}:{day}",
                    lane.as_str()
                );
                let notice = Notice {
                    kind: NotificationKind::OrganizationAlert,
                    subject_user_id: None,
                    title: &title,
                    body: &body,
                    metadata: json!({
                        "lane": lane.as_str(),
                        "refused": counted.count,
                        "since": counted.since,
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
                        told_set.retain(|(_, _, d)| *d >= today.pred_opt().unwrap_or(today));
                        told_set.insert(told);
                    }
                    Err(error) => tracing::warn!(
                        organization_id = %organization_id,
                        lane = lane.as_str(),
                        error = %error,
                        "telemetry: a day's refusals could not be told; the next pass tries again"
                    ),
                }
            }
        }
    }
}

/// [`tell`], every five minutes, forever; a pass that panics ends itself
/// alone.
pub async fn run(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(TELL_EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let pass = tokio::spawn({
            let state = state.clone();
            async move { tell(&state).await }
        });
        if let Err(error) = pass.await {
            tracing::error!(error = %error, "telemetry: a pass telling refusals died; the next one runs");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two pings a run, so a run every five minutes needs two fifths of a
    /// ping a minute, and five of them exactly two.
    #[test]
    fn a_schedule_needs_two_pings_a_run() {
        assert_eq!(need_of_gap(300), 2 * MICRO_PINGS / 5);
        assert_eq!(5 * need_of_gap(300), 2 * MICRO_PINGS);
        assert_eq!(need_of_gap(60), 2 * MICRO_PINGS);
        assert_eq!(need_of_gap(420), 285_715, "rounded up, never down");
        assert_eq!(need_of_gap(0), 120 * MICRO_PINGS, "no gap is the most");
    }

    #[test]
    fn a_ceiling_reads_as_plans_md_writes_it_and_midnight_is_never_now() {
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

    /// A notice names the lane, the count and the time it began, and a
    /// monitors notice the slugs, and never a person.
    #[test]
    fn each_lanes_notice_says_how_many_and_since_when() {
        let refused = Refused {
            count: 14,
            since: DateTime::parse_from_rfc3339("2026-10-22T14:02:00Z")
                .expect("a time")
                .with_timezone(&Utc),
        };
        let slugs: BTreeSet<String> = ["nightly-sync".to_owned()].into();
        let (_, body) = notice_text(Lane::Pings, &refused, &slugs);
        assert!(
            body.contains("refused 14 times") && body.contains("14:02 UTC on 22 October"),
            "{body}"
        );
        let once = Refused {
            count: 1,
            ..refused.clone()
        };
        let (_, body) = notice_text(Lane::Pings, &once, &slugs);
        assert!(body.contains("refused once"), "{body}");
        let (_, body) = notice_text(Lane::DataOut, &refused, &slugs);
        assert!(
            body.contains("14 times") && body.contains("midnight UTC"),
            "{body}"
        );
        let (_, body) = notice_text(Lane::Monitors, &refused, &slugs);
        assert!(body.contains("nightly-sync"), "{body}");
    }
}
