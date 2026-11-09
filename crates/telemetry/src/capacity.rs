//! What each organization's plan holds of what it spends, held as it is
//! spent: the span units its agents send over its month, counted at ingest's
//! door in Redis, shared by every replica. Past them, a plan with a grace
//! keeps every span for its days while the owner is asked to lift it; then,
//! and from the first span over where there is none, one run in
//! [`SAMPLE_ONE_IN`] is kept whole, each of its spans flagged with the rate,
//! and the rest are counted in the answer's partial success, nothing
//! dropped unsaid. The numbers are the deployment's, read through the
//! entitlements seam and kept a minute per organization; a binary that sells
//! nothing answers none, and nothing is held or counted. Redis out of reach
//! counts nothing and holds nothing back: an outage of ours never stops an
//! agent reporting. Past its content ceiling, an export's spans keep their
//! metadata alone; and each night every replica holds each organization to
//! its plan's retention and its content ceiling ([`purge`]), each step
//! idempotent, so a pass two replicas run takes nothing twice.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, DurationRound as _, Utc};
use redis::AsyncCommands;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::seam::{self, Entitlements, OrganizationAlert};
use telmoni_shared::{Capacity, OrganizationId};

use telmoni_shared::ProjectId;
use telmoni_shared::db::tenant_session::{maintenance_scope, organization_scope};

use crate::AppState;
use crate::db::{self, TelemetryLane};
use crate::handler::chain;
use crate::hashing::UserIdStorage;
use crate::ingest::batch::Batch;
use crate::ingest::span::DropReason;
use crate::retention::{CONTENT_BATCH, SPAN_RETENTION_DAYS, cutoff_after, midnight};
use crate::store;

/// How long an organization's capacity is kept: a plan bought or left is
/// held to within a minute.
const KEEP_FOR: Duration = Duration::from_secs(60);

/// How many organizations' answers are kept at once.
const KNOWN_MAX: usize = 10_000;

/// Past a plan's spans, one run in this many is kept.
pub const SAMPLE_ONE_IN: u64 = 10;

/// How long a month's counts outlive the longest month, so a late export
/// still finds them.
const COUNTS_KEPT_S: i64 = 40 * 24 * 60 * 60;

/// The capacities as one replica holds them.
#[derive(Default)]
pub struct Capacities {
    known: Mutex<HashMap<OrganizationId, (Option<Capacity>, Instant)>>,
}

impl Capacities {
    /// The organization's capacity, from memory for a minute after the seam
    /// last answered.
    pub async fn of(
        &self,
        entitlements: &dyn Entitlements,
        organization_id: &OrganizationId,
    ) -> Option<Capacity> {
        if let Some((capacity, at)) = self
            .known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(organization_id)
            && at.elapsed() < KEEP_FOR
        {
            return capacity.clone();
        }
        let capacity = seam::capacity(entitlements, organization_id).await;
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        if known.len() >= KNOWN_MAX {
            known.retain(|_, (_, at)| at.elapsed() < KEEP_FOR);
        }
        known.insert(organization_id.clone(), (capacity.clone(), Instant::now()));
        capacity
    }
}

/// What an export was held to past its plan: the sentence its sampled-out
/// spans were told with, and whether its content was past the ceiling. Its
/// answer says so, and a copy sent again is answered the same.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Held {
    pub sampled: Option<String>,
    pub content_full: bool,
}

/// Whether a run is among those kept past a plan's spans: chosen by its id
/// under the project's own key, so every span of a run, in any export, is
/// kept or sampled out together, and no sender can choose ids that are kept.
#[must_use]
pub fn kept_past_the_grant(storage: &UserIdStorage, run_id: Uuid) -> bool {
    let digest = storage.store(&format!("sample:{run_id}"));
    digest
        .get(..16)
        .and_then(|head| u64::from_str_radix(head, 16).ok())
        .is_some_and(|n| n.is_multiple_of(SAMPLE_ONE_IN))
}

/// The days back a read of the project may reach: what was asked, held to
/// its organization's retention, so what a move to a plan holding less keeps
/// for its hold is hidden until it goes. A count that cannot be read holds
/// nothing back.
pub async fn visible_days(state: &AppState, project_id: &ProjectId, days: u16) -> u16 {
    let filed = async {
        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&state.db, project_id).await?;
        let organization = db::filed_under(&mut tx, project_id).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(organization)
    };
    let Ok(Some(organization_id)) = filed.await else {
        return days;
    };
    match state
        .capacities
        .of(state.entitlements.as_ref(), &organization_id)
        .await
    {
        Some(capacity) => days.min(u16::try_from(capacity.retention_days).unwrap_or(u16::MAX)),
        None => days,
    }
}

/// The month's counts an export's kept spans go on once written, by
/// [`count_spans`]: the organization's, and its project's where capped.
pub struct Month {
    keys: Vec<String>,
}

/// What of the batch the organization's month has room for, the rest
/// sampled out, their content with them, and counted in its report; the
/// month the spans kept are counted against once the export is written,
/// so a write that fails is not counted for its retry as well; and the
/// sentence they were sampled out with, if any were.
pub async fn hold_spans(
    state: &Arc<AppState>,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    batch: &mut Batch,
    storage: &UserIdStorage,
    now: DateTime<Utc>,
) -> (Option<Month>, Option<String>) {
    if batch.rows.is_empty() {
        return (None, None);
    }
    let (organization_key, mut sampled) =
        hold_organization_spans(state, organization_id, batch, storage, now).await;
    let (project_key, project_sampled) = hold_project_spans(
        state,
        organization_id,
        project_id,
        batch,
        storage,
        now,
        sampled.is_some(),
    )
    .await;
    sampled = sampled.or(project_sampled);
    let keys: Vec<String> = organization_key.into_iter().chain(project_key).collect();
    ((!keys.is_empty()).then_some(Month { keys }), sampled)
}

/// What of the batch the organization's month has room for: the key its
/// month is counted under, and the sentence the rest were sampled out with.
async fn hold_organization_spans(
    state: &Arc<AppState>,
    organization_id: &OrganizationId,
    batch: &mut Batch,
    storage: &UserIdStorage,
    now: DateTime<Utc>,
) -> (Option<String>, Option<String>) {
    let Some(capacity) = state
        .capacities
        .of(state.entitlements.as_ref(), organization_id)
        .await
    else {
        return (None, None);
    };
    let Some(mut connection) = state.redis.connection().await else {
        return (None, None);
    };
    let month = capacity.period_start.timestamp();
    // The grace and its telling are the allowance's: one raised starts both again.
    let limit = capacity.spans.limit;
    let used_key = format!("telemetry:spans:{organization_id}:{month}");
    let over_key = format!("telemetry:spans-over:{organization_id}:{month}:{limit}");
    let told_key = format!("telemetry:spans-told:{organization_id}:{month}:{limit}");
    let used: Result<Option<u64>, redis::RedisError> = connection.get(&used_key).await;
    let used = match used {
        Ok(used) => used.unwrap_or(0),
        Err(error) => {
            state.redis.stood_in("the month's spans", &error);
            return (None, None);
        }
    };
    let month = Some(used_key);
    if capacity.spans.has_room(used) {
        return (month, None);
    }
    // The first export over marks when the grace began; the owners are told
    // by whichever export first claims the telling, and again by a later one
    // where that telling failed.
    let marked: Result<(Option<i64>, bool), redis::RedisError> = redis::pipe()
        .atomic()
        .set_nx(&over_key, now.timestamp())
        .ignore()
        .expire(&over_key, COUNTS_KEPT_S)
        .ignore()
        .get(&over_key)
        .set_nx(&told_key, 1)
        .expire(&told_key, COUNTS_KEPT_S)
        .ignore()
        .query_async(&mut connection)
        .await;
    let over_since = match marked {
        Ok((since, tell)) => {
            if tell {
                tell_over(Arc::clone(state), organization_id, &capacity, told_key);
            }
            since
                .and_then(|at| DateTime::from_timestamp(at, 0))
                .unwrap_or(now)
        }
        Err(error) => {
            state.redis.stood_in("the month's spans", &error);
            return (month, None);
        }
    };
    let grace = chrono::Duration::days(i64::from(capacity.spans_grace_days));
    if now - over_since < grace {
        return (month, None);
    }
    sample(batch, &capacity.spans.detail, storage);
    (month, Some(capacity.spans.detail))
}

/// What of the batch the project's own cap on spans a month has room for,
/// where its owners and admins set one: past it, sampled with no grace,
/// since the cap is theirs, and the owner and admins told once. Answers the
/// key its month is counted under, and the sentence of any it sampled out.
/// A batch the organization's month already sampled is counted, not sampled
/// again.
async fn hold_project_spans(
    state: &Arc<AppState>,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    batch: &mut Batch,
    storage: &UserIdStorage,
    now: DateTime<Utc>,
    sampled: bool,
) -> (Option<String>, Option<String>) {
    let Some(cap) = state.project_caps.of(state, project_id).await.spans() else {
        return (None, None);
    };
    let period = project_period(state, organization_id, now).await;
    let Some(mut connection) = state.redis.connection().await else {
        return (None, None);
    };
    let used_key = format!("telemetry:project-spans:{project_id}:{period}");
    let used: Result<Option<u64>, redis::RedisError> = connection.get(&used_key).await;
    let used = match used {
        Ok(used) => used.unwrap_or(0),
        Err(error) => {
            state.redis.stood_in("a project's month of spans", &error);
            return (None, None);
        }
    };
    if used < cap || sampled {
        return (Some(used_key), None);
    }
    let told_key = format!("telemetry:project-spans-told:{project_id}:{period}:{cap}");
    let claimed: Result<(bool,), redis::RedisError> = redis::pipe()
        .atomic()
        .set_nx(&told_key, 1)
        .expire(&told_key, COUNTS_KEPT_S)
        .ignore()
        .query_async(&mut connection)
        .await;
    if let Ok((true,)) = claimed {
        crate::project_limits::tell(
            state,
            organization_id,
            project_id,
            "spans a month",
            cap,
            period.to_string(),
            Some(told_key),
        )
        .await;
    }
    let detail = crate::project_limits::detail("spans a month", cap);
    sample(batch, &detail, storage);
    (Some(used_key), Some(detail))
}

/// When the month a project's spans are counted over began, as a timestamp:
/// its organization's own month where a plan answers one, else the UTC
/// calendar month.
pub async fn project_period(
    state: &AppState,
    organization_id: &OrganizationId,
    now: DateTime<Utc>,
) -> i64 {
    if let Some(capacity) = state
        .capacities
        .of(state.entitlements.as_ref(), organization_id)
        .await
    {
        return capacity.period_start.timestamp();
    }
    use chrono::Datelike as _;
    now.date_naive()
        .with_day(1)
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .map_or(now.timestamp(), |start| start.and_utc().timestamp())
}

/// The units of `rows`, written and kept, counted against the month.
pub async fn count_spans(state: &AppState, month: Month, rows: &[crate::store::SpanRow]) {
    let units: u64 = rows.iter().map(|row| u64::from(row.units)).sum();
    if units == 0 {
        return;
    }
    let Some(mut connection) = state.redis.connection().await else {
        return;
    };
    let mut counting = redis::pipe();
    counting.atomic();
    for key in &month.keys {
        counting
            .incr(key, units)
            .ignore()
            .expire(key, COUNTS_KEPT_S)
            .ignore();
    }
    let counted: Result<(), redis::RedisError> = counting.query_async(&mut connection).await;
    if let Err(error) = counted {
        state.redis.stood_in("the month's spans", &error);
    }
}

/// Whether the organization's content held is under its plan's ceiling:
/// what a file uploaded alone, and an export's content, are kept under. A
/// count that cannot be read keeps it, as an outage of ours never drops
/// what came.
pub async fn has_room_for_content(state: &AppState, organization_id: &OrganizationId) -> bool {
    let Some(capacity) = state
        .capacities
        .of(state.entitlements.as_ref(), organization_id)
        .await
    else {
        return true;
    };
    match content_held(state, organization_id).await {
        Ok(held) => held < capacity.content_bytes,
        Err(error) => {
            tracing::warn!(organization_id = %organization_id, error = %error,
                "telemetry: the content held could not be read; what came is kept");
            true
        }
    }
}

/// The content an export brings, kept while the organization holds less than
/// its plan's ceiling and the project less than its own cap; past either,
/// every field is dropped, named `content_full`
/// in the answer, and the spans keep their metadata alone. Answers whether it
/// was past.
pub async fn hold_content(
    state: &Arc<AppState>,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    batch: &mut Batch,
) -> bool {
    if batch.content.is_empty() {
        return false;
    }
    if !has_room_for_content(state, organization_id).await {
        drop_content(batch);
        return true;
    }
    // Past the project's own cap, its owners and admins told once a cap.
    if !crate::project_limits::has_room_for_content(state, project_id).await {
        if let Some(cap) = state
            .project_caps
            .of(state, project_id)
            .await
            .content_bytes()
            && let Some(told_key) =
                crate::project_limits::claim(state, project_id, "bytes of content", cap, "held")
                    .await
        {
            crate::project_limits::tell(
                state,
                organization_id,
                project_id,
                "bytes of content",
                cap,
                "held".to_owned(),
                Some(told_key),
            )
            .await;
        }
        drop_content(batch);
        return true;
    }
    false
}

/// Every field of the batch's content dropped, each named `content_full`.
pub fn drop_content(batch: &mut Batch) {
    for span in std::mem::take(&mut batch.content) {
        for (field, _) in &span.fields {
            batch
                .report
                .dropped
                .push(&span.span_id, field, DropReason::ContentFull);
        }
    }
}

/// One run in [`SAMPLE_ONE_IN`] kept, each of its spans flagged with the rate
/// it was kept at; the rest, their content and what they dropped, counted in
/// the answer with the sentence that names what lifts it.
pub fn sample(batch: &mut Batch, detail: &str, storage: &UserIdStorage) {
    let before = batch.rows.len();
    batch
        .rows
        .retain(|row| kept_past_the_grant(storage, row.run_id));
    batch
        .content
        .retain(|content| kept_past_the_grant(storage, content.run_id));
    #[expect(clippy::cast_precision_loss, reason = "a sampling rate, ten at most")]
    let rate = 1.0 / SAMPLE_ONE_IN as f64;
    for row in &mut batch.rows {
        row.sample_rate = Some(rate);
    }
    let kept: std::collections::HashSet<&str> =
        batch.rows.iter().map(|row| row.span_id.as_str()).collect();
    batch
        .report
        .dropped
        .retain_spans(|span_id| kept.contains(span_id));
    let sampled_out = before - batch.rows.len();
    batch.report.sample_out(sampled_out, detail);
}

/// The owner and admins told, once a month, that the organization's spans
/// are past its plan's, in the plan's own words for what lifts them; a
/// telling that fails gives up its claim, so a later export tells again.
fn tell_over(
    state: Arc<AppState>,
    organization_id: &OrganizationId,
    capacity: &Capacity,
    told_key: String,
) {
    let organization_id = organization_id.clone();
    let month = capacity.period_start;
    let grace = capacity.spans_grace_days;
    let limit = capacity.spans.limit;
    let detail = capacity.spans.detail.clone();
    tokio::spawn(async move {
        if !chain::open(state.auth.as_ref(), &organization_id).await {
            return;
        }
        let body = if grace > 0 {
            format!(
                "{detail} Every span is kept for {grace} more days; after that, one run in \
                 {SAMPLE_ONE_IN} is kept, until the month renews."
            )
        } else {
            format!(
                "{detail} Until the month renews, one run in {SAMPLE_ONE_IN} is kept and the \
                 rest are sampled out."
            )
        };
        let dedup_key = format!(
            "telemetry:spans-over:{organization_id}:{}:{}",
            month.timestamp(),
            limit
        );
        let alert = OrganizationAlert {
            title: "Spans past the plan's month",
            body: &body,
            metadata: json!({ "lane": "spans", "month": month, "grace_days": grace }),
            dedup_key: Some(&dedup_key),
        };
        let path = organization_path(&state, &organization_id).await;
        let told = seam::tell_owners(
            state.notifications.as_ref(),
            state.auth.as_ref(),
            &organization_id,
            alert,
            &path,
        )
        .await;
        // A telling that failed gives up its claim, so a later export tells.
        if !told && let Some(mut connection) = state.redis.connection().await {
            let _: Result<(), redis::RedisError> = connection.del(&told_key).await;
        }
    });
}

/// The usage pass's name in `telemetry.heartbeats`.
pub const USAGE_HEARTBEAT: &str = "usage";

/// How often the month's spans are counted again from the hourly totals.
const USAGE_EVERY: Duration = Duration::from_secs(60 * 60);

/// The span units `projects` landed in the hours from one to another, from the
/// hourly totals.
const MONTH_UNITS: &str = "SELECT toUInt64(sum(units)) FROM telemetry.spans_hourly \
     WHERE project_id IN {projects:Array(String)} AND hour >= {since:DateTime('UTC')} \
       AND hour < {until:DateTime('UTC')}";

/// The organization's span units this month: the door's count, or the hourly
/// totals of the hours wholly past where they hold more, as after Redis lost
/// its count. With `raise`, the count is set to theirs in one step that never
/// lowers it; an export landing this hour is counted by the door alone, so
/// none is counted twice.
pub async fn spans_used(
    state: &AppState,
    organization_id: &OrganizationId,
    capacity: &Capacity,
    raise: bool,
) -> Result<u64, telmoni_shared::TelmoniError> {
    let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
    let projects = db::projects_filed(&mut lane, organization_id).await?;
    lane.commit().await?;
    let since = capacity.period_start;
    let totals = hours_past(state, projects, since).await?;
    let key = format!("telemetry:spans:{organization_id}:{}", since.timestamp());
    let Some(mut connection) = state.redis.connection().await else {
        return Ok(totals);
    };
    let counted: Result<Option<u64>, redis::RedisError> = connection.get(&key).await;
    let counted = counted
        .unwrap_or_else(|error| {
            state.redis.stood_in("the month's spans", &error);
            None
        })
        .unwrap_or(0);
    if raise && totals > counted {
        let raised: Result<(), redis::RedisError> = redis::Script::new(RAISE)
            .key(&key)
            .arg(totals)
            .arg(COUNTS_KEPT_S)
            .invoke_async(&mut connection)
            .await;
        if let Err(error) = raised {
            state.redis.stood_in("the month's spans", &error);
        }
    }
    Ok(counted.max(totals))
}

/// The span units of `projects` in the hours wholly past since `since`, by the
/// hourly totals: the hour under way is the door's alone to count.
async fn hours_past(
    state: &AppState,
    projects: Vec<ProjectId>,
    since: DateTime<Utc>,
) -> Result<u64, telmoni_shared::TelmoniError> {
    let this_hour = Utc::now()
        .duration_trunc(chrono::Duration::hours(1))
        .unwrap_or_else(|_| Utc::now());
    let Some(read) = store::Projects::of(projects) else {
        return Ok(0);
    };
    state
        .store
        .read(&read, MONTH_UNITS)
        .param("since", store::seconds(since))
        .param("until", store::seconds(this_hour))
        .fetch_one::<u64>()
        .await
        .map_err(|error| {
            telmoni_shared::TelmoniError::Internal(format!(
                "the month's spans could not be read ({})",
                store::Fault::of(&error).as_str()
            ))
        })
}

/// A project's count of its month raised to its hours wholly past, never
/// lowered. Nothing counts a project without a span cap, so a cap set
/// mid-period starts from here, as does a count Redis lost; what the hour
/// under way sent before the cap is not recovered.
pub async fn raise_project_spans(
    state: &AppState,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
) -> Result<(), telmoni_shared::TelmoniError> {
    let period = project_period(state, organization_id, Utc::now()).await;
    let since = DateTime::from_timestamp(period, 0).unwrap_or_else(Utc::now);
    let totals = hours_past(state, vec![project_id.clone()], since).await?;
    if totals == 0 {
        return Ok(());
    }
    let Some(mut connection) = state.redis.connection().await else {
        return Ok(());
    };
    let raised: Result<(), redis::RedisError> = redis::Script::new(RAISE)
        .key(format!("telemetry:project-spans:{project_id}:{period}"))
        .arg(totals)
        .arg(COUNTS_KEPT_S)
        .invoke_async(&mut connection)
        .await;
    if let Err(error) = raised {
        state.redis.stood_in("a project's month of spans", &error);
    }
    Ok(())
}

/// A count set to `ARGV[1]` where it holds less, never lowered.
const RAISE: &str = "local held = tonumber(redis.call('GET', KEYS[1]) or '0') \
     if held < tonumber(ARGV[1]) then redis.call('SET', KEYS[1], ARGV[1], 'EX', ARGV[2]) end \
     return 1";

/// [`tell_usage`], every hour, forever.
pub async fn run_usage(state: Arc<AppState>) {
    state
        .heartbeats
        .every(&state.db, USAGE_HEARTBEAT, START_DELAY, USAGE_EVERY, || {
            let state = state.clone();
            async move { tell_usage(&state).await }
        })
        .await;
}

/// Each organization's capped projects counted again, whatever its plan; and
/// where a plan sells capacity, its month counted again and its owners told,
/// on its feed and by mail, once its spans reach four fifths of the month and
/// once its content fills its ceiling; the month's spans spent are told at the
/// door ([`hold_spans`]). One organization that cannot be read holds up no other.
pub async fn tell_usage(state: &AppState) -> Result<(), telmoni_shared::TelmoniError> {
    let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
    let filed = db::organizations_filed(&mut lane).await?;
    lane.commit().await?;
    for (organization_id, _) in filed {
        // First, and whatever the plan: a project's own cap holds where no
        // plan sells capacity too, as on a self-hosted installation.
        raise_capped_projects(state, &organization_id).await;
        let Some(capacity) = seam::capacity(state.entitlements.as_ref(), &organization_id).await
        else {
            continue;
        };
        let month = capacity.period_start.timestamp();
        let renews = capacity.period_end.format("%-d %B");
        let limit = capacity.spans.limit;
        match spans_used(state, &organization_id, &capacity, true).await {
            Ok(used) if seam_share(used, limit) == Some(80) => {
                if !chain::open(state.auth.as_ref(), &organization_id).await {
                    continue;
                }
                let body = format!(
                    "This organization has sent four fifths of the spans its plan holds this \
                     month, which renews on {renews}. {}",
                    capacity
                        .spans
                        .detail
                        .split_once(". ")
                        .map_or("", |(_, cure)| cure)
                );
                tell(
                    state,
                    &organization_id,
                    "Spans at four fifths of the month",
                    &body,
                    &format!("telemetry:usage:{organization_id}:spans:{month}:{limit}:80"),
                )
                .await;
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(organization_id = %organization_id, error = %error,
                "telemetry: an organization's month could not be counted; the next hour tries"),
        }
        // Content held is not a month's: told once it is full, until the
        // ceiling it was full against changes.
        match content_held(state, &organization_id).await {
            Ok(held) if held >= capacity.content_bytes => {
                if !chain::open(state.auth.as_ref(), &organization_id).await {
                    continue;
                }
                let ceiling = capacity.content_bytes;
                tell(
                    state,
                    &organization_id,
                    "Content held",
                    "This organization holds all the content its plan does: a span arriving \
                     keeps its metadata alone, and each night the oldest content goes first, \
                     until a tenth is free.",
                    &format!("telemetry:usage:{organization_id}:content:{ceiling}:100"),
                )
                .await;
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(organization_id = %organization_id, error = %error,
                "telemetry: an organization's content held could not be read"),
        }
    }
    Ok(())
}

/// Each of the organization's projects with a span cap, its count raised to
/// its hours wholly past. One that cannot be read holds up no other.
async fn raise_capped_projects(state: &AppState, organization_id: &OrganizationId) {
    let projects = async {
        let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
        let projects = db::projects_filed(&mut lane, organization_id).await?;
        lane.commit().await?;
        Ok::<_, telmoni_shared::TelmoniError>(projects)
    };
    let projects = match projects.await {
        Ok(projects) => projects,
        Err(error) => {
            tracing::warn!(organization_id = %organization_id, error = %error,
                "telemetry: an organization's projects could not be read to count their months");
            return;
        }
    };
    for project_id in projects {
        if state
            .project_caps
            .of(state, &project_id)
            .await
            .spans()
            .is_none()
        {
            continue;
        }
        if let Err(error) = raise_project_spans(state, organization_id, &project_id).await {
            tracing::warn!(project_id = %project_id, error = %error,
                "telemetry: a project's month could not be counted; the next hour tries");
        }
    }
}

/// The share told of `used` out of `limit`.
fn seam_share(used: u64, limit: u64) -> Option<u8> {
    telmoni_shared::types::share_told(used, limit)
}

/// The owners told once by `dedup_key`, on the feed and by mail, the
/// organization's own page linked.
async fn tell(
    state: &AppState,
    organization_id: &OrganizationId,
    title: &str,
    body: &str,
    dedup_key: &str,
) {
    let path = organization_path(state, organization_id).await;
    let alert = OrganizationAlert {
        title,
        body,
        metadata: json!({ "lane": "usage" }),
        dedup_key: Some(dedup_key),
    };
    seam::tell_owners(
        state.notifications.as_ref(),
        state.auth.as_ref(),
        organization_id,
        alert,
        &path,
    )
    .await;
}

/// The organization's page in the console, by its slug; the console's root
/// where auth cannot say it.
async fn organization_path(state: &AppState, organization_id: &OrganizationId) -> String {
    match state
        .auth
        .organization_slugs(std::slice::from_ref(organization_id))
        .await
    {
        Ok(slugs) => slugs
            .into_iter()
            .next()
            .map_or_else(|| "/".to_owned(), |(_, slug)| format!("/{slug}")),
        Err(_) => "/".to_owned(),
    }
}

/// The projects among those read that hold a span before the cutoff.
const OLD_SPANS: &str = "SELECT DISTINCT project_id FROM telemetry.spans \
     WHERE project_id IN {projects:Array(String)} \
       AND received_at < {cutoff:DateTime64(6, 'UTC')}";

/// The nightly pass's name in `telemetry.heartbeats`.
pub const HEARTBEAT: &str = "capacity";

/// How often the pass runs: each night.
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// How long after boot the first pass runs, past the first probes.
const START_DELAY: Duration = Duration::from_secs(10 * 60);

/// What one pass took.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Purged {
    /// Projects whose spans past their organization's retention went.
    pub projects: u64,
    /// Content rows that went, past retention or past a ceiling.
    pub content: u64,
}

/// [`purge`], each night, forever.
pub async fn run(state: Arc<AppState>) {
    state
        .heartbeats
        .every(&state.db, HEARTBEAT, START_DELAY, EVERY, || {
            let state = state.clone();
            async move {
                let purged = purge(&state, Utc::now()).await?;
                if purged != Purged::default() {
                    tracing::info!(
                        projects = purged.projects,
                        content = purged.content,
                        "telemetry: the night's pass held organizations to their plans"
                    );
                }
                Ok::<(), telmoni_shared::TelmoniError>(())
            }
        })
        .await;
}

/// Every organization with a project filed here held to its plan: spans and
/// content past its retention deleted, and past its content ceiling the
/// oldest content deleted, a day at a time, until a tenth of it is free. One
/// organization that cannot be held is logged and holds up no other.
pub async fn purge(
    state: &AppState,
    now: DateTime<Utc>,
) -> Result<Purged, telmoni_shared::TelmoniError> {
    let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
    let filed = db::organizations_filed(&mut lane).await?;
    lane.commit().await?;
    let mut purged = Purged::default();
    for (organization_id, projects) in filed {
        let Some(capacity) = seam::capacity(state.entitlements.as_ref(), &organization_id).await
        else {
            continue;
        };
        if let Err(error) = hold(
            state,
            &organization_id,
            &projects,
            &capacity,
            now,
            &mut purged,
        )
        .await
        {
            tracing::warn!(organization_id = %organization_id, error = %error,
                "telemetry: an organization could not be held to its plan tonight; the next \
                 night tries again");
        }
    }
    Ok(purged)
}

/// One organization's projects held to its plan.
async fn hold(
    state: &AppState,
    organization_id: &OrganizationId,
    projects: &[ProjectId],
    capacity: &Capacity,
    now: DateTime<Utc>,
    purged: &mut Purged,
) -> Result<(), telmoni_shared::TelmoniError> {
    // A fall to a plan holding less waits out its hold before anything
    // the plan before it kept goes.
    if capacity.held_until.is_some_and(|until| now < until) {
        return Ok(());
    }
    let days = i64::from(capacity.retention_days);
    if days < SPAN_RETENTION_DAYS {
        let cutoff = cutoff_after(now, days);
        let fault = |error: clickhouse::error::Error| {
            telmoni_shared::TelmoniError::Internal(format!(
                "the spans past retention were not purged ({})",
                store::Fault::of(&error).as_str()
            ))
        };
        // A delete is sent only where a span past the cutoff is.
        let past = match store::Projects::of(projects.iter().cloned()) {
            Some(read) => state
                .store
                .read(&read, OLD_SPANS)
                .param("cutoff", store::micros(cutoff))
                .fetch_all::<String>()
                .await
                .map_err(fault)?,
            None => Vec::new(),
        };
        for project in projects
            .iter()
            .filter(|p| past.iter().any(|old| old == p.as_str()))
        {
            state
                .store
                .delete_before(project, cutoff)
                .await
                .map_err(fault)?;
            purged.projects += 1;
        }
        purged.content += delete_content_before(state, projects, cutoff).await?;
        let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
        crate::changes::purge_before(lane.conn(), Some(projects), cutoff).await?;
        lane.commit().await?;
    }
    let mut held = content_held(state, organization_id).await?;
    if held <= capacity.content_bytes {
        return Ok(());
    }
    let room = capacity.content_bytes - capacity.content_bytes / 10;
    let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
    let oldest = db::oldest_content(&mut lane, projects).await?;
    lane.commit().await?;
    let Some(oldest) = oldest else {
        return Ok(());
    };
    let mut day = oldest.date_naive();
    while held > room && midnight(day) <= now {
        day = day.succ_opt().unwrap_or(day);
        purged.content += delete_content_before(state, projects, midnight(day)).await?;
        held = content_held(state, organization_id).await?;
    }
    Ok(())
}

/// The organization's content held, in sealed bytes.
pub(crate) async fn content_held(
    state: &AppState,
    organization_id: &OrganizationId,
) -> Result<u64, telmoni_shared::TelmoniError> {
    let mut tx = organization_scope(&state.db, organization_id).await?;
    let held = db::content_held(&mut tx, organization_id).await?;
    tx.commit().await?;
    Ok(held)
}

/// `projects`' content received before `cutoff`, a batch a transaction, its
/// bytes taken off their organization's count.
async fn delete_content_before(
    state: &AppState,
    projects: &[ProjectId],
    cutoff: DateTime<Utc>,
) -> Result<u64, telmoni_shared::TelmoniError> {
    let mut gone = 0;
    loop {
        let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
        let batch = db::delete_content(lane.conn(), projects, Some(cutoff), CONTENT_BATCH).await?;
        lane.commit().await?;
        gone += batch;
        if batch < CONTENT_BATCH.unsigned_abs() {
            return Ok(gone);
        }
    }
}
