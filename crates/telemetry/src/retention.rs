//! The nightly purge of ClickHouse's partitions, which `telmoni rotate` runs
//! as the migrator's user. Retention is a purge, not a filter: a span's day
//! goes whole once every row in it is older than the longest retention sold,
//! an hour's totals once their month is older than
//! [`ROLLUP_RETENTION_MONTHS`], each partition listed from the rows, so a
//! night missed is caught up by the next. A deleted project's rows, marked by
//! its purge and gone from every read within moments as the store applies
//! the mark ([`crate::store::Store::delete_project`]), leave the disk by
//! the next night's run ([`rewrite_deleted`]).

use anyhow::Context;
use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, TimeZone, Utc};

use crate::schema::STATEMENT_TIMEOUT;
use crate::store::within;

/// The longest retention sold, in days: a span's day goes whole once every
/// row in it is older than this.
pub const SPAN_RETENTION_DAYS: i64 = 360;

/// How long the hourly totals are kept, in months: they name no user and no
/// tag, so a baseline keeps a year behind it.
pub const ROLLUP_RETENTION_MONTHS: u32 = 13;

/// The days of spans wholly before the cutoff: the partitions to drop.
const EXPIRED_SPAN_DAYS: &str = "SELECT DISTINCT _partition_id FROM telemetry.spans \
     WHERE received_at < {cutoff:DateTime64(6, 'UTC')} ORDER BY _partition_id";

const DROP_SPAN_DAY: &str = "ALTER TABLE telemetry.spans DROP PARTITION ID {partition:String}";

/// The months of totals wholly before the cutoff.
const EXPIRED_ROLLUP_MONTHS: &str = "SELECT DISTINCT _partition_id FROM telemetry.spans_hourly \
     WHERE hour < {cutoff:DateTime('UTC')} ORDER BY _partition_id";

const DROP_ROLLUP_MONTH: &str =
    "ALTER TABLE telemetry.spans_hourly DROP PARTITION ID {partition:String}";

/// How long the server may spend on a listing, in seconds: stopped short of
/// the client's own limit on the statement, so the night's log says why.
const LIST_MAX_EXECUTION_SECS: &str = "50";

/// The mask a lightweight delete wrote, applied: the rows a deleted project
/// left are rewritten out of the parts that hold them, and no other part is
/// touched.
const APPLY_SPAN_MASK: &str = "ALTER TABLE telemetry.spans APPLY DELETED MASK";

const APPLY_ROLLUP_MASK: &str = "ALTER TABLE telemetry.spans_hourly APPLY DELETED MASK";

/// What one purge dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Purged {
    /// Days of spans.
    pub span_days: usize,
    /// Months of hourly totals.
    pub rollup_months: usize,
}

/// The first instant `spans` keeps at `now`: midnight UTC of the day
/// [`SPAN_RETENTION_DAYS`] before. Each day's partition wholly before it goes.
/// A date out of range keeps everything rather than drop what it should not.
#[must_use]
pub fn span_cutoff(now: DateTime<Utc>) -> DateTime<Utc> {
    midnight(
        now.date_naive()
            .checked_sub_signed(Duration::days(SPAN_RETENTION_DAYS))
            .unwrap_or(NaiveDate::MIN),
    )
}

/// The first instant `spans_hourly` keeps at `now`: the first of the month
/// [`ROLLUP_RETENTION_MONTHS`] before. Each month's partition wholly before
/// it goes. A date out of range keeps everything, as above.
#[must_use]
pub fn rollup_cutoff(now: DateTime<Utc>) -> DateTime<Utc> {
    midnight(
        now.date_naive()
            .with_day(1)
            .and_then(|first| first.checked_sub_months(Months::new(ROLLUP_RETENTION_MONTHS)))
            .unwrap_or(NaiveDate::MIN),
    )
}

fn midnight(day: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&day.and_time(chrono::NaiveTime::MIN))
}

/// Drop every day of spans and every month of totals past retention, then
/// rewrite away the rows deleted projects left, as `client`'s user — the
/// migrator's, whose own row policy admits every row, so the listing sees
/// every project's. Idempotent: a partition already gone is not listed, and
/// a mask already applied rewrites nothing. Every step runs whichever failed
/// before it; the first failure is the answer.
pub async fn purge(client: &clickhouse::Client, now: DateTime<Utc>) -> anyhow::Result<Purged> {
    let span_days = drop_before(
        client,
        "spans",
        EXPIRED_SPAN_DAYS,
        DROP_SPAN_DAY,
        &span_cutoff(now).format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
    )
    .await
    .context("dropping days of spans past retention");
    let rollup_months = drop_before(
        client,
        "spans_hourly",
        EXPIRED_ROLLUP_MONTHS,
        DROP_ROLLUP_MONTH,
        &rollup_cutoff(now).format("%Y-%m-%d %H:%M:%S").to_string(),
    )
    .await
    .context("dropping months of hourly totals past retention");
    let rewritten = rewrite_deleted(client)
        .await
        .context("rewriting away the rows deleted projects left");
    let purged = Purged {
        span_days: span_days?,
        rollup_months: rollup_months?,
    };
    rewritten?;
    Ok(purged)
}

/// Apply the deleted mask on both tables, as the migrator, waiting on each
/// rewrite as long as a statement may take. One that takes longer is left
/// running — a mutation finishes on its own, and the next night finds
/// nothing left to rewrite — and noted; any other failure is the answer,
/// once both tables have been tried.
pub async fn rewrite_deleted(client: &clickhouse::Client) -> anyhow::Result<()> {
    let mut first_failure = None;
    for (table, statement) in [
        ("spans", APPLY_SPAN_MASK),
        ("spans_hourly", APPLY_ROLLUP_MASK),
    ] {
        match within(
            STATEMENT_TIMEOUT,
            client
                .query_raw(statement)
                .with_setting("mutations_sync", "1")
                .execute(),
        )
        .await
        {
            Ok(()) => {
                tracing::info!(
                    table,
                    "rotate: the rows deleted projects left are off the disk"
                );
            }
            Err(clickhouse::error::Error::TimedOut) => {
                tracing::info!(
                    table,
                    "rotate: the rows deleted projects left are still being rewritten away; \
                     the mutation finishes on its own"
                );
            }
            Err(e) => {
                tracing::error!(
                    table,
                    error = %e,
                    "rotate: the deleted mask could not be applied"
                );
                first_failure.get_or_insert(e);
            }
        }
    }
    match first_failure {
        None => Ok(()),
        Some(e) => Err(e.into()),
    }
}

/// List the partitions of `table` that `list` finds before `cutoff`, and
/// drop each with `drop`. A drop that fails is logged and the rest still
/// run: the listing is in partition order, so a partition that cannot go —
/// one over the server's `max_partition_size_to_drop`, say — would otherwise
/// hold back every one after it, night after night. The first failure is the
/// answer, once every drop has been tried.
async fn drop_before(
    client: &clickhouse::Client,
    table: &'static str,
    list: &str,
    drop: &str,
    cutoff: &str,
) -> anyhow::Result<usize> {
    let partitions: Vec<String> = match within(
        STATEMENT_TIMEOUT,
        client
            .query_raw(list)
            .with_setting("max_execution_time", LIST_MAX_EXECUTION_SECS)
            .param("cutoff", cutoff)
            .fetch_all(),
    )
    .await
    {
        Ok(partitions) => partitions,
        Err(e) => {
            // Logged here too, since the night answers its first failure
            // alone, and this table's may not be it.
            tracing::error!(
                table,
                error = %e,
                "rotate: the expired ClickHouse partitions could not be listed"
            );
            return Err(e.into());
        }
    };
    let mut first_failure = None;
    let mut dropped = 0;
    for partition in &partitions {
        match within(
            STATEMENT_TIMEOUT,
            client
                .query_raw(drop)
                .param("partition", partition)
                .execute(),
        )
        .await
        {
            Ok(()) => {
                dropped += 1;
                tracing::info!(
                    table,
                    partition = %partition,
                    "rotate: dropped an expired ClickHouse partition"
                );
            }
            Err(e) => {
                tracing::error!(
                    table,
                    partition = %partition,
                    error = %e,
                    "rotate: an expired ClickHouse partition could not be dropped"
                );
                first_failure.get_or_insert(e);
            }
        }
    }
    match first_failure {
        None => Ok(dropped),
        Some(e) => Err(anyhow::Error::new(e).context(format!(
            "{} of {} expired partitions of {table} could not be dropped",
            partitions.len() - dropped,
            partitions.len()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339).unwrap().to_utc()
    }

    /// A row is kept its full 360 days: its day goes only once the cutoff
    /// has passed the end of it.
    #[test]
    fn a_span_day_goes_once_every_row_in_it_is_past_retention() {
        let now = at("2027-10-09T02:00:00Z");
        let cutoff = span_cutoff(now);
        assert_eq!(cutoff, at("2026-10-14T00:00:00Z"));
        let last_row_of_the_first_kept_day = at("2026-10-14T23:59:59Z");
        assert!(last_row_of_the_first_kept_day >= cutoff);
        let last_row_of_the_last_dropped_day = at("2026-10-13T23:59:59Z");
        assert!(last_row_of_the_last_dropped_day < cutoff);
        assert!(
            now - last_row_of_the_last_dropped_day >= Duration::days(SPAN_RETENTION_DAYS),
            "a row went before its 360 days"
        );
    }

    /// The totals are kept thirteen months at the least: a month goes once
    /// the month thirteen months on has ended.
    #[test]
    fn a_month_of_totals_goes_once_it_is_thirteen_months_behind() {
        assert_eq!(
            rollup_cutoff(at("2027-11-09T02:00:00Z")),
            at("2026-10-01T00:00:00Z")
        );
        assert_eq!(
            rollup_cutoff(at("2027-01-31T23:00:00Z")),
            at("2025-12-01T00:00:00Z")
        );
        let last_hour_of_a_dropped_month = at("2026-09-30T23:00:00Z");
        let first_night_it_goes = at("2027-11-01T02:00:00Z");
        assert!(last_hour_of_a_dropped_month < rollup_cutoff(first_night_it_goes));
        assert!(
            last_hour_of_a_dropped_month >= rollup_cutoff(at("2027-10-31T02:00:00Z")),
            "a month went before its thirteen months"
        );
    }
}
