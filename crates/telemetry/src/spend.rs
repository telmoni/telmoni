//! What runs spent: each model call's cost as reported, else its tokens at
//! the price in force the day it arrived ([`crate::prices`]), else nothing,
//! the call named as unpriced. Each read first finds the models of its calls
//! that reported no cost, then is handed their prices as parameters, so
//! ClickHouse keeps no copy of a price, and one corrected in the price file
//! corrects every read after the next `migrate`.

use clickhouse::Row;
use serde::Deserialize;
use uuid::Uuid;

use telmoni_shared::{ProjectId, RuleAggregation};

use crate::AppState;
use crate::prices::PriceBook;
use crate::rules::RuleFilters;
use crate::rules::evaluate::Window;
use crate::store::{Projects, Read, micros};

/// Each call's spend at the row for its name last in force by its day, its project's
/// own first ([`PriceBook::rows`]); unpriced where no cost, no row, and tokens to price.
macro_rules! priced {
    () => {
        concat!(
            "arrayLastIndex((j, p, m, f) -> (j = '' OR j = project_id) AND p = provider ",
            "AND m = model AND f <= toDate(received_at), ",
            "{price_projects:Array(String)}, {price_providers:Array(String)}, ",
            "{price_models:Array(String)}, {price_from:Array(Date)}) AS price_row, ",
            "ifNull(cost, if(price_row = 0, 0, ",
            "toFloat64(if(input_tokens > cache_read_input_tokens + cache_write_input_tokens, ",
            "input_tokens - cache_read_input_tokens - cache_write_input_tokens, 0)) ",
            "* {price_input:Array(Float64)}[price_row] ",
            "+ toFloat64(cache_read_input_tokens) * {price_cache_read:Array(Float64)}[price_row] ",
            "+ toFloat64(cache_write_input_tokens) * {price_cache_write:Array(Float64)}[price_row] ",
            "+ toFloat64(output_tokens) * {price_output:Array(Float64)}[price_row])) AS spent, ",
            "isNull(cost) AND price_row = 0 AND (input_tokens > 0 OR output_tokens > 0 ",
            "OR cache_read_input_tokens > 0 OR cache_write_input_tokens > 0) AS unpriced"
        )
    };
}

/// One span's spend as an expression, for a sum over many: its cost as
/// reported, else its tokens at its price row, else 0, as [`priced!`]'s.
macro_rules! spent_of {
    () => {
        concat!(
            "ifNull(cost, if((arrayLastIndex((j, p, m, f) -> (j = '' OR j = project_id) ",
            "AND p = provider AND m = model AND f <= toDate(received_at), ",
            "{price_projects:Array(String)}, {price_providers:Array(String)}, ",
            "{price_models:Array(String)}, {price_from:Array(Date)}) AS row_priced) = 0, 0, ",
            "toFloat64(if(input_tokens > cache_read_input_tokens + cache_write_input_tokens, ",
            "input_tokens - cache_read_input_tokens - cache_write_input_tokens, 0)) ",
            "* {price_input:Array(Float64)}[row_priced] ",
            "+ toFloat64(cache_read_input_tokens) * {price_cache_read:Array(Float64)}[row_priced] ",
            "+ toFloat64(cache_write_input_tokens) * {price_cache_write:Array(Float64)}[row_priced] ",
            "+ toFloat64(output_tokens) * {price_output:Array(Float64)}[row_priced]))"
        )
    };
}

pub(crate) use spent_of;

/// One row of the hourly totals' spend, `h` the table: its costs as
/// reported where they were, else its tokens at its price row by its day.
macro_rules! rollup_spent_of {
    () => {
        concat!(
            "if(h.cost_reported, h.cost, if((arrayLastIndex((j, p, m, f) -> ",
            "(j = '' OR j = h.project_id) AND p = h.provider AND m = h.model AND f <= toDate(h.hour), ",
            "{price_projects:Array(String)}, {price_providers:Array(String)}, ",
            "{price_models:Array(String)}, {price_from:Array(Date)}) AS rollup_priced) = 0, 0, ",
            "toFloat64(if(h.input_tokens > h.cache_read_input_tokens + h.cache_write_input_tokens, ",
            "h.input_tokens - h.cache_read_input_tokens - h.cache_write_input_tokens, 0)) ",
            "* {price_input:Array(Float64)}[rollup_priced] ",
            "+ toFloat64(h.cache_read_input_tokens) * {price_cache_read:Array(Float64)}[rollup_priced] ",
            "+ toFloat64(h.cache_write_input_tokens) * {price_cache_write:Array(Float64)}[rollup_priced] ",
            "+ toFloat64(h.output_tokens) * {price_output:Array(Float64)}[rollup_priced]))"
        )
    };
}

pub(crate) use rollup_spent_of;

/// The names the hourly totals from `from` price: their rows of calls that
/// reported no cost.
pub(crate) const ROLLUP_NAMES_SQL: &str = concat!(
    "SELECT DISTINCT h.provider AS provider, h.model AS model FROM telemetry.spans_hourly AS h ",
    "WHERE h.project_id IN {projects:Array(String)} AND h.hour >= {from:DateTime('UTC')} ",
    "AND NOT h.cost_reported AND h.provider != '' AND h.model != ''"
);

/// The prices the hourly totals from `from` are read at, `projects`' own
/// among them.
///
/// # Errors
///
/// ClickHouse or Postgres did not answer, or refused the read.
pub async fn book_for_rollups(
    state: &AppState,
    projects: &Projects,
    from: chrono::DateTime<chrono::Utc>,
) -> Result<PriceBook, Unread> {
    let names: Vec<Name> = state
        .store
        .read(projects, ROLLUP_NAMES_SQL)
        .param("from", crate::store::seconds(from))
        .fetch_all()
        .await?;
    Ok(PriceBook::read_for(&state.db, projects.as_slice(), &pairs(names)).await?)
}

/// The model calls a window holds that an alert's filters admit, which take
/// no tool or status for a run's cost.
macro_rules! window_calls {
    () => {
        concat!(
            "FROM telemetry.spans ",
            "WHERE project_id IN {projects:Array(String)} ",
            "AND received_at >= {from:DateTime64(6, 'UTC')} ",
            "AND received_at < {to:DateTime64(6, 'UTC')} ",
            "AND kind = 'model_call' ",
            $crate::rules::calls_admitted!(),
            // A run's tags ride on its own span, landing as it ends: found from a day before.
            "AND (empty({tags:Array(String)}) OR run_id IN (SELECT run_id FROM telemetry.spans ",
            "WHERE project_id IN {projects:Array(String)} ",
            "AND received_at >= {from:DateTime64(6, 'UTC')} - INTERVAL 1 DAY ",
            "AND hasAny(arrayMap((k, v) -> concat(k, ':', v), mapKeys(tags), mapValues(tags)), ",
            "{tags:Array(String)}))) ",
        )
    };
}

/// The names a window's calls that reported no cost are priced by.
const WINDOW_NAMES_SQL: &str = concat!(
    "SELECT DISTINCT provider, model ",
    window_calls!(),
    " AND isNull(cost) AND provider != '' AND model != ''"
);

/// Each run's spend over a window, folded: how many runs, the most one spent,
/// their average, their sum scaled by each call's sample rate, and the costliest.
const RUNS_SQL: &str = concat!(
    "SELECT count() AS runs, max(run_spent) AS most, avg(run_spent) AS mean, ",
    "sum(run_scaled) AS total, argMax(run_id, run_spent) AS costliest ",
    "FROM (SELECT run_id, sum(spent) AS run_spent, ",
    "sum(spent / ifNull(sample_rate, 1)) AS run_scaled ",
    "FROM (SELECT run_id, sample_rate, ",
    priced!(),
    " ",
    window_calls!(),
    ") GROUP BY run_id)"
);

/// The names one run's calls that reported no cost are priced by.
const RUN_NAMES_SQL: &str = concat!(
    "SELECT DISTINCT provider, model FROM telemetry.spans ",
    "WHERE project_id IN {projects:Array(String)} AND run_id = {run:UUID} ",
    "AND kind = 'model_call' AND isNull(cost) AND provider != '' AND model != ''"
);

/// What one run has spent so far, and the names of its calls with neither a
/// cost nor a price.
const RUN_SQL: &str = concat!(
    "SELECT sum(spent) AS run_spent, ",
    "arraySort(groupUniqArrayIf((provider, model), unpriced)) AS unpriced_names ",
    "FROM (SELECT provider, model, ",
    priced!(),
    " FROM telemetry.spans ",
    "WHERE project_id IN {projects:Array(String)} AND run_id = {run:UUID} ",
    "AND kind = 'model_call')"
);

/// The names a period's calls that reported no cost are priced by.
const PERIOD_NAMES_SQL: &str = concat!(
    "SELECT DISTINCT provider, model FROM telemetry.spans ",
    "WHERE project_id IN {projects:Array(String)} ",
    "AND received_at >= {from:DateTime64(6, 'UTC')} AND received_at < {to:DateTime64(6, 'UTC')} ",
    "AND kind = 'model_call' AND isNull(cost) AND provider != '' AND model != ''"
);

/// What each project's calls spent over a period, each scaled by its sample
/// rate, a project with none left out: what an organization's Overview shows.
const PROJECTS_SQL: &str = concat!(
    "SELECT project_id, sum(spent / ifNull(sample_rate, 1)) AS spent, ",
    "toUInt64(countIf(unpriced)) AS unpriced_calls ",
    "FROM (SELECT project_id, sample_rate, ",
    priced!(),
    " FROM telemetry.spans ",
    "WHERE project_id IN {projects:Array(String)} ",
    "AND received_at >= {from:DateTime64(6, 'UTC')} AND received_at < {to:DateTime64(6, 'UTC')} ",
    "AND kind = 'model_call') GROUP BY project_id ORDER BY spent DESC, project_id"
);

/// One project's spend over a period.
#[derive(Debug, Clone, PartialEq, Row, Deserialize)]
pub struct ProjectSpent {
    pub project_id: String,
    /// In US dollars, each call reported or priced.
    pub spent: f64,
    /// Its calls with neither a cost nor a price.
    pub unpriced_calls: u64,
}

/// What each of `projects`' calls spent from `from` to `to`, the costliest
/// first, a project that made no model call left out.
///
/// # Errors
///
/// ClickHouse or Postgres did not answer, or refused the read.
pub async fn projects(
    state: &AppState,
    projects: &Projects,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<ProjectSpent>, Unread> {
    let between = |read: Read| read.param("from", micros(from)).param("to", micros(to));
    let names: Vec<Name> = between(state.store.read(projects, PERIOD_NAMES_SQL))
        .fetch_all()
        .await?;
    let book = PriceBook::read_for(&state.db, projects.as_slice(), &pairs(names)).await?;
    Ok(
        priced_by(between(state.store.read(projects, PROJECTS_SQL)), &book)
            .fetch_all()
            .await?,
    )
}

/// Why a read of spend went unanswered: ClickHouse, whose error a caller
/// logs by its kind alone ([`crate::store`]), or Postgres, which holds the
/// prices.
#[derive(Debug)]
pub enum Unread {
    Store(clickhouse::error::Error),
    Db(sqlx::Error),
}

impl From<clickhouse::error::Error> for Unread {
    fn from(error: clickhouse::error::Error) -> Self {
        Self::Store(error)
    }
}

impl From<sqlx::Error> for Unread {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error)
    }
}

/// What the runs of a window spent, folded.
#[derive(Debug, Clone, Default, PartialEq, Row, Deserialize)]
pub struct RunsSpent {
    /// The runs whose model calls the window holds.
    pub runs: u64,
    /// The most one of them spent.
    pub most: f64,
    /// Their average.
    pub mean: f64,
    /// Their sum.
    pub total: f64,
    /// The run that spent the most.
    #[serde(with = "clickhouse::serde::uuid")]
    pub costliest: Uuid,
}

impl RunsSpent {
    /// An alert's value: the fold `aggregation` names, none where the window
    /// holds no run's call.
    #[must_use]
    pub fn value(&self, aggregation: RuleAggregation) -> Option<f64> {
        if self.runs == 0 {
            return None;
        }
        match aggregation {
            RuleAggregation::Max => Some(self.most),
            RuleAggregation::Avg => Some(self.mean),
            RuleAggregation::Sum => Some(self.total),
            // A run's cost folds no other way ([`crate::rules::folds`]).
            _ => None,
        }
    }
}

/// What one run has spent so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunSpent {
    /// In US dollars, each call reported or priced.
    pub spent: f64,
    /// The provider and model of each name its calls with neither a cost
    /// nor a price carry, in order.
    pub unpriced: Vec<(String, String)>,
}

#[derive(Row, Deserialize)]
struct Name {
    provider: String,
    model: String,
}

#[derive(Row, Deserialize)]
struct Spent {
    run_spent: f64,
    unpriced_names: Vec<(String, String)>,
}

/// What each run of `projects` spent over `window`, read from its model
/// calls that `filters` admit, folded.
///
/// # Errors
///
/// ClickHouse or Postgres did not answer, or refused the read.
pub async fn runs(
    state: &AppState,
    projects: &Projects,
    window: Window,
    filters: &RuleFilters,
) -> Result<RunsSpent, Unread> {
    let names: Vec<Name> = windowed(
        state.store.read(projects, WINDOW_NAMES_SQL),
        window,
        filters,
    )
    .fetch_all()
    .await?;
    let book = PriceBook::read_for(&state.db, projects.as_slice(), &pairs(names)).await?;
    Ok(priced_by(
        windowed(state.store.read(projects, RUNS_SQL), window, filters),
        &book,
    )
    .fetch_one()
    .await?)
}

/// What run `run_id` of `projects` has spent so far, all of its calls read.
///
/// # Errors
///
/// ClickHouse or Postgres did not answer, or refused the read.
pub async fn run(state: &AppState, projects: &Projects, run_id: Uuid) -> Result<RunSpent, Unread> {
    let names: Vec<Name> = state
        .store
        .read(projects, RUN_NAMES_SQL)
        .param("run", run_id)
        .fetch_all()
        .await?;
    let book = PriceBook::read_for(&state.db, projects.as_slice(), &pairs(names)).await?;
    let spent: Spent = priced_by(
        state.store.read(projects, RUN_SQL).param("run", run_id),
        &book,
    )
    .fetch_one()
    .await?;
    Ok(RunSpent {
        spent: spent.run_spent,
        unpriced: spent.unpriced_names,
    })
}

/// The names a list's window prices: its calls that reported no cost.
const LIST_NAMES_SQL: &str = concat!(
    "SELECT DISTINCT provider, model FROM telemetry.spans ",
    "WHERE project_id IN {projects:Array(String)} ",
    "AND received_at >= now64(6) - toIntervalDay({days:UInt16}) ",
    "AND kind = 'model_call' AND isNull(cost) AND provider != '' AND model != ''"
);

/// The prices a list over the last `days` reads its spend at, its projects'
/// own among them.
///
/// # Errors
///
/// ClickHouse or Postgres did not answer, or refused the read.
pub async fn book_for_days(
    state: &AppState,
    projects: &Projects,
    days: u16,
) -> Result<PriceBook, Unread> {
    let names: Vec<Name> = state
        .store
        .read(projects, LIST_NAMES_SQL)
        .param("days", days)
        .fetch_all()
        .await?;
    Ok(PriceBook::read_for(&state.db, projects.as_slice(), &pairs(names)).await?)
}

fn pairs(names: Vec<Name>) -> Vec<(String, String)> {
    names
        .into_iter()
        .map(|Name { provider, model }| (provider, model))
        .collect()
}

/// `read` bound to `window` and to the filters a run's cost takes.
fn windowed(read: Read, window: Window, filters: &RuleFilters) -> Read {
    filters
        .bind_calls(
            read.param("from", micros(window.from))
                .param("to", micros(window.to)),
        )
        .param("tags", &filters.tag)
}

/// `read` handed `book`'s prices in the order they take effect, the table's as
/// project `''`, a price for always from the epoch's first day.
pub(crate) fn priced_by(read: Read, book: &PriceBook) -> Read {
    let (projects, rows): (Vec<&str>, Vec<_>) = book
        .rows()
        .map(|(project, row)| (project.map_or("", ProjectId::as_str), row))
        .unzip();
    let column = |price: fn(&crate::prices::ModelPrice) -> f64| {
        rows.iter().map(|row| price(row)).collect::<Vec<f64>>()
    };
    read.param("price_projects", projects)
        .param(
            "price_providers",
            rows.iter()
                .map(|row| row.provider.as_str())
                .collect::<Vec<_>>(),
        )
        .param(
            "price_models",
            rows.iter()
                .map(|row| row.model.as_str())
                .collect::<Vec<_>>(),
        )
        .param(
            "price_from",
            rows.iter()
                .map(|row| {
                    row.effective_from
                        .map_or_else(|| "1970-01-01".to_owned(), |day| day.to_string())
                })
                .collect::<Vec<_>>(),
        )
        .param("price_input", column(|row| row.input))
        .param("price_output", column(|row| row.output))
        .param("price_cache_read", column(|row| row.cache_read))
        .param("price_cache_write", column(|row| row.cache_write))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run's cost folds to the most, the average or the sum, and to
    /// nothing where no run's call landed.
    #[test]
    fn a_runs_cost_is_folded_as_its_alert_asks() {
        let spent = RunsSpent {
            runs: 4,
            most: 5.5,
            mean: 2.0,
            total: 8.0,
            costliest: Uuid::nil(),
        };
        assert_eq!(spent.value(RuleAggregation::Max), Some(5.5));
        assert_eq!(spent.value(RuleAggregation::Avg), Some(2.0));
        assert_eq!(spent.value(RuleAggregation::Sum), Some(8.0));
        assert_eq!(spent.value(RuleAggregation::P99), None);
        assert_eq!(RunsSpent::default().value(RuleAggregation::Max), None);
    }

    /// Every read binds the prices it reads, and each price is bound once.
    #[test]
    fn the_reads_name_the_prices_they_are_handed() {
        for sql in [RUNS_SQL, RUN_SQL] {
            for price in [
                "{price_projects:Array(String)}",
                "{price_providers:Array(String)}",
                "{price_models:Array(String)}",
                "{price_from:Array(Date)}",
                "{price_input:Array(Float64)}",
                "{price_output:Array(Float64)}",
                "{price_cache_read:Array(Float64)}",
                "{price_cache_write:Array(Float64)}",
            ] {
                assert_eq!(sql.matches(price).count(), 1, "{price} in {sql}");
            }
        }
    }
}
