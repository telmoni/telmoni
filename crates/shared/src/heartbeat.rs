//! Each loop's heartbeat, which the external watchdog reads through the
//! server's `GET /heartbeat`: a row a loop in its module's own `heartbeats`,
//! written by whichever replica finished a tick of it. A loop is stale once no
//! replica has beaten it for [`MISSED_PERIODS`] of its periods, each at least
//! [`PERIOD_FLOOR`], the watchdog's own minute, so a loop that ticks every few
//! seconds writes twice a minute rather than every tick.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use sqlx::PgPool;

use crate::db::tenant_session::{Lane, maintenance_scope};

/// How many of its periods a loop may go unbeaten before it is stale.
pub const MISSED_PERIODS: u32 = 3;

/// The shortest period a loop is held to: the watchdog asks once a minute.
pub const PERIOD_FLOOR: Duration = Duration::from_secs(60);

/// One loop, as this replica runs it.
struct Loop {
    period: Duration,
    /// When this replica last wrote its beat.
    written: Option<Instant>,
}

/// The loops one module runs on this replica, and the table they beat in.
pub struct Heartbeats<L> {
    /// The module's schema, which holds its `heartbeats`.
    schema: &'static str,
    lane: L,
    loops: Mutex<HashMap<&'static str, Loop>>,
}

impl<L: Lane + Copy> Heartbeats<L> {
    /// No loop yet, beating in `<schema>.heartbeats` through `lane`.
    #[must_use]
    pub fn new(schema: &'static str, lane: L) -> Self {
        Self {
            schema,
            lane,
            loops: Mutex::new(HashMap::new()),
        }
    }

    /// `name` runs on this replica, a tick every `period`: from now on the
    /// watchdog reads it.
    pub fn runs(&self, name: &'static str, period: Duration) {
        self.with_loops(|loops| {
            loops.insert(
                name,
                Loop {
                    period: period.max(PERIOD_FLOOR),
                    written: None,
                },
            );
        });
    }

    /// `name` finished a tick: its beat written, unless this replica wrote one
    /// within half a [`PERIOD_FLOOR`]. A beat that does not land is logged,
    /// and the loop reads as stale until one does.
    pub async fn beat(&self, pool: &PgPool, name: &'static str) {
        let due = self.with_loops(|loops| {
            let one = loops.get_mut(name)?;
            if one
                .written
                .is_some_and(|at| at.elapsed() < PERIOD_FLOOR / 2)
            {
                return None;
            }
            one.written = Some(Instant::now());
            Some(one.period)
        });
        let Some(period) = due else {
            return;
        };
        if let Err(error) = self.write(pool, name, period).await {
            self.with_loops(|loops| {
                if let Some(one) = loops.get_mut(name) {
                    one.written = None;
                }
            });
            tracing::warn!(
                schema = self.schema,
                loop_name = name,
                error = %error,
                "a loop's heartbeat was not written; the watchdog reads it as stale until one is"
            );
        }
    }

    /// The loops this replica runs that no replica has beaten for
    /// [`MISSED_PERIODS`] of their periods, each `<schema>.<name>`, sorted;
    /// every one of them where the table cannot be read, since none can be
    /// shown beating.
    pub async fn stale(&self, pool: &PgPool) -> Vec<String> {
        let (names, after): (Vec<&'static str>, Vec<i64>) = self.with_loops(|loops| {
            loops
                .iter()
                .map(|(name, one)| {
                    let missed = one.period.saturating_mul(MISSED_PERIODS);
                    (*name, i64::try_from(missed.as_secs()).unwrap_or(i64::MAX))
                })
                .unzip()
        });
        if names.is_empty() {
            return Vec::new();
        }
        let stale = match self.read_stale(pool, &names, &after).await {
            Ok(stale) => stale,
            Err(error) => {
                tracing::warn!(
                    schema = self.schema,
                    error = %error,
                    "the loops' heartbeats could not be read; the watchdog is told every one is stale"
                );
                let mut every: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
                every.sort_unstable();
                every
            }
        };
        stale
            .into_iter()
            .map(|name| format!("{}.{name}", self.schema))
            .collect()
    }

    async fn write(&self, pool: &PgPool, name: &str, period: Duration) -> sqlx::Result<()> {
        let schema = self.schema;
        let mut tx = maintenance_scope(pool, self.lane).await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO {schema}.heartbeats (name, period_seconds, beat_at) VALUES ($1, $2, now()) \
             ON CONFLICT (name) DO UPDATE \
             SET period_seconds = EXCLUDED.period_seconds, beat_at = EXCLUDED.beat_at"
        )))
        .bind(name)
        .bind(i32::try_from(period.as_secs()).unwrap_or(i32::MAX))
        .execute(tx.conn())
        .await?;
        tx.commit().await
    }

    async fn read_stale(
        &self,
        pool: &PgPool,
        names: &[&str],
        after: &[i64],
    ) -> sqlx::Result<Vec<String>> {
        let schema = self.schema;
        let mut tx = maintenance_scope(pool, self.lane).await?;
        let stale: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT ran.name FROM unnest($1::text[], $2::bigint[]) AS ran (name, after) \
             LEFT JOIN {schema}.heartbeats AS beaten ON beaten.name = ran.name \
             WHERE beaten.beat_at IS NULL \
                OR beaten.beat_at < now() - make_interval(secs => ran.after) \
             ORDER BY ran.name"
        )))
        .bind(names)
        .bind(after)
        .fetch_all(tx.conn())
        .await?;
        tx.commit().await?;
        Ok(stale)
    }

    fn with_loops<T>(&self, f: impl FnOnce(&mut HashMap<&'static str, Loop>) -> T) -> T {
        f(&mut self.loops.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
