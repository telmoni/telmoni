//! The console's trace lanes, under `Resource::Trace`, which every role on a
//! project holds: `GET /internal/v1/telemetry/traces`, the list — a page of rows,
//! a row per run in a window of days, filtered and sorted as the URL says,
//! and how many rows there are; `/traces/facets`, each filter's values and
//! how many runs carry each; `/traces/histogram`, the runs by when they
//! started; `/traces/export`, the rows as a file; and `/traces/{run_id}`, one
//! trace's spans in the order they started. Each reads the spans of the
//! project `x-project-id` names and no other: the store names that project on
//! every read, twice.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use uuid::Uuid;

use telmoni_shared::csv;
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuthError, SortDirection, SpanKind, SpanStatus, TelemetryError, TelmoniError, TraceSort,
};

use crate::AppState;
use crate::contract::{
    AGENT_MAX, ATTRIBUTE_VALUE_MAX, ENVIRONMENT_MAX, MODEL_MAX, NAME_MAX, RELEASE_MAX,
    SESSION_ID_MAX, TAG_KEY_MAX, TAG_VALUE_MAX, USER_ID_MAX,
};
use crate::store::{self, Projects, SpanRow};
use crate::traces::{
    self, Bar, DEFAULT_LIMIT, EXPORT_MAX, FACET_VALUES_MAX, FILTER_VALUES_MAX, FacetRow, MAX_DAYS,
    MAX_LIMIT, MAX_PAGE, RUNS_MAX, SPANS_MAX, TRACE_UNITS_MAX, TraceFilter, TraceRow,
};

/// Which lane a query string is read for. Each takes the filters and the
/// window, and its own parameters beside them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// The list: its sort, and its page.
    List,
    /// The facets: the filters alone.
    Facets,
    /// The histogram: the reader's clock.
    Histogram,
    /// A download: its sort, its format, and the runs chosen for it.
    Export,
}

/// A trace lane's query string, every value text until it is checked. A
/// filter may be given again, each value one more the list admits, and an
/// empty value — a form's untouched field — admits nothing; every other
/// parameter is given once at most, and one the lane does not take is a 400.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TraceQuery {
    /// The runs' agents.
    pub agent: Vec<String>,
    /// Their end users, as stored.
    pub user: Vec<String>,
    /// Their sessions, by id.
    pub session: Vec<String>,
    /// How they ended: `ok`, `error` or `cancelled`.
    pub status: Vec<String>,
    /// Models one of their calls answered with.
    pub model: Vec<String>,
    /// Tools one of their calls ran.
    pub tool: Vec<String>,
    /// The deployments their spans were sent from.
    pub environment: Vec<String>,
    /// The releases their spans were sent from.
    pub release: Vec<String>,
    /// Tags any of their spans carry, each `key:value`.
    pub tag: Vec<String>,
    /// Prompt names any of their spans carry.
    pub prompt: Vec<String>,
    /// Runs by id: a download's chosen rows.
    pub run: Vec<String>,
    /// The column to sort by ([`TraceSort`]).
    pub sort: Option<String>,
    /// Which way ([`SortDirection`]); the column's own default when unset.
    pub dir: Option<String>,
    /// How many days back, 1 to [`MAX_DAYS`].
    pub days: Option<String>,
    /// The most traces a page holds, 1 to [`MAX_LIMIT`].
    pub limit: Option<String>,
    /// Which page, from 1 to [`MAX_PAGE`].
    pub page: Option<String>,
    /// The reader's clock, in whole minutes east of UTC.
    pub utc_offset: Option<String>,
    /// A download's format: `csv` or `json`.
    pub format: Option<String>,
}

impl TraceQuery {
    /// The query string's pairs, read for `reading`'s lane.
    ///
    /// # Errors
    ///
    /// A parameter the lane does not take, or one given twice that is not a
    /// filter. A name the lane does not know is never echoed: a person typed
    /// it.
    pub fn read(pairs: Vec<(String, String)>, reading: Reading) -> Result<Self, TelmoniError> {
        use Reading::{Export, Histogram, List};
        let mut query = Self::default();
        for (name, value) in pairs {
            let once = match (name.as_str(), reading) {
                ("agent", _) => {
                    query.agent.push(value);
                    continue;
                }
                ("user", _) => {
                    query.user.push(value);
                    continue;
                }
                ("session", _) => {
                    query.session.push(value);
                    continue;
                }
                ("status", _) => {
                    query.status.push(value);
                    continue;
                }
                ("model", _) => {
                    query.model.push(value);
                    continue;
                }
                ("tool", _) => {
                    query.tool.push(value);
                    continue;
                }
                ("environment", _) => {
                    query.environment.push(value);
                    continue;
                }
                ("release", _) => {
                    query.release.push(value);
                    continue;
                }
                ("tag", _) => {
                    query.tag.push(value);
                    continue;
                }
                ("prompt", _) => {
                    query.prompt.push(value);
                    continue;
                }
                ("run", Export) => {
                    query.run.push(value);
                    continue;
                }
                ("days", _) => &mut query.days,
                ("sort", List | Export) => &mut query.sort,
                ("dir", List | Export) => &mut query.dir,
                ("limit", List) => &mut query.limit,
                ("page", List) => &mut query.page,
                ("utc_offset", Histogram) => &mut query.utc_offset,
                ("format", Export) => &mut query.format,
                _ => {
                    return Err(TelmoniError::BadRequest(
                        "the lane takes no parameter by that name".into(),
                    ));
                }
            };
            if once.replace(value).is_some() {
                return Err(TelmoniError::BadRequest(format!(
                    "{name} is given more than once"
                )));
            }
        }
        Ok(query)
    }

    /// The filter the query asks for, each value checked.
    ///
    /// # Errors
    ///
    /// A value that is not what its parameter takes, or more values than one
    /// filter admits, named by the parameter and never echoed.
    pub fn filter(&self) -> Result<TraceFilter, TelmoniError> {
        let statuses = values("status", &self.status, usize::MAX)?
            .iter()
            .map(|given| {
                given.parse::<SpanStatus>().map_err(|_| {
                    TelmoniError::BadRequest("status is one of ok, error, cancelled".into())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let runs = values_up_to("run", &self.run, usize::MAX, RUNS_MAX)?
            .iter()
            .map(|given| {
                given.parse::<Uuid>().map_err(|_| {
                    TelmoniError::BadRequest("a run is named by its id, a UUID".into())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TraceFilter {
            agents: values("agent", &self.agent, AGENT_MAX)?,
            user_ids: values("user", &self.user, USER_ID_MAX)?,
            session_ids: values("session", &self.session, SESSION_ID_MAX)?,
            statuses,
            models: values("model", &self.model, MODEL_MAX)?,
            tools: values("tool", &self.tool, NAME_MAX)?,
            environments: values("environment", &self.environment, ENVIRONMENT_MAX)?,
            releases: values("release", &self.release, RELEASE_MAX)?,
            tags: tag_values(&self.tag)?,
            prompts: values("prompt", &self.prompt, ATTRIBUTE_VALUE_MAX)?,
            runs,
            days: parse_days(self.days.as_deref().unwrap_or_default())?,
        })
    }

    /// The sort the query asks for, each column its own way unless the
    /// query says which.
    ///
    /// # Errors
    ///
    /// A column or a direction there is none of, never echoed.
    pub fn sort(&self) -> Result<(TraceSort, SortDirection), TelmoniError> {
        let sort = match self.sort.as_deref().unwrap_or_default().trim() {
            "" => TraceSort::Started,
            given => given.parse::<TraceSort>().map_err(|_| {
                TelmoniError::BadRequest(
                    "sort is one of started, agent, user, session, status, steps, model_calls, \
                     tool_calls, errors, input_tokens, output_tokens, tokens, duration"
                        .into(),
                )
            })?,
        };
        let direction = match self.dir.as_deref().unwrap_or_default().trim() {
            "" => default_direction(sort),
            given => given
                .parse::<SortDirection>()
                .map_err(|_| TelmoniError::BadRequest("dir is asc or desc".into()))?,
        };
        Ok((sort, direction))
    }
}

/// A filter's values, trimmed: an empty one dropped, as a form's untouched
/// field, and one given twice read once.
pub(crate) fn values(
    name: &str,
    given: &[String],
    max: usize,
) -> Result<Vec<String>, TelmoniError> {
    values_up_to(name, given, max, FILTER_VALUES_MAX)
}

/// A parameter's values as [`values`] reads them, `most` of them at most:
/// the runs a download names are a page's rows, more than a filter admits.
fn values_up_to(
    name: &str,
    given: &[String],
    max: usize,
    most: usize,
) -> Result<Vec<String>, TelmoniError> {
    let mut kept: Vec<String> = Vec::new();
    for value in given {
        let value = value.trim();
        if value.is_empty() || kept.iter().any(|k| k == value) {
            continue;
        }
        if value.chars().count() > max {
            return Err(TelmoniError::BadRequest(format!(
                "{name} is longer than {max} characters"
            )));
        }
        kept.push(value.to_owned());
    }
    if kept.len() > most {
        return Err(TelmoniError::BadRequest(format!(
            "{name} is given more than {most} values"
        )));
    }
    Ok(kept)
}

/// A tag filter's values, each `key:value` as ingest keeps a tag: the key 1
/// to [`TAG_KEY_MAX`] characters, the value up to [`TAG_VALUE_MAX`].
fn tag_values(given: &[String]) -> Result<Vec<String>, TelmoniError> {
    let pairs = values("tag", given, TAG_KEY_MAX + 1 + TAG_VALUE_MAX)?;
    for pair in &pairs {
        let well_formed = pair.split_once(':').is_some_and(|(key, value)| {
            (1..=TAG_KEY_MAX).contains(&key.chars().count())
                && value.chars().count() <= TAG_VALUE_MAX
        });
        if !well_formed {
            return Err(TelmoniError::BadRequest(format!(
                "tag is key:value, the key 1 to {TAG_KEY_MAX} characters and the value up to \
                 {TAG_VALUE_MAX}"
            )));
        }
    }
    Ok(pairs)
}

/// The way a column sorts when the URL names none: the newest, the longest
/// and the most first; names in the alphabet's order.
#[must_use]
pub const fn default_direction(sort: TraceSort) -> SortDirection {
    match sort {
        TraceSort::Started
        | TraceSort::Steps
        | TraceSort::ModelCalls
        | TraceSort::ToolCalls
        | TraceSort::Errors
        | TraceSort::InputTokens
        | TraceSort::OutputTokens
        | TraceSort::Tokens
        | TraceSort::Duration => SortDirection::Desc,
        TraceSort::Agent | TraceSort::User | TraceSort::Session | TraceSort::Status => {
            SortDirection::Asc
        }
    }
}

/// `days` as a URL carries it: a whole number from 1 to [`MAX_DAYS`], and
/// the default when empty; every page over the spans reads its window so.
pub(crate) fn parse_days(text: &str) -> Result<u16, TelmoniError> {
    match text.trim() {
        "" => Ok(traces::DEFAULT_DAYS),
        given => given
            .parse::<u16>()
            .ok()
            .filter(|days| (1..=MAX_DAYS).contains(days))
            .ok_or_else(|| {
                TelmoniError::BadRequest(format!("days is a whole number from 1 to {MAX_DAYS}"))
            }),
    }
}

/// `limit` as a URL carries it: a whole number from 1 to [`MAX_LIMIT`], and
/// the default when empty.
pub(crate) fn parse_limit(text: &str) -> Result<u32, TelmoniError> {
    match text.trim() {
        "" => Ok(DEFAULT_LIMIT),
        given => given
            .parse::<u32>()
            .ok()
            .filter(|limit| (1..=MAX_LIMIT).contains(limit))
            .ok_or_else(|| {
                TelmoniError::BadRequest(format!("limit is a whole number from 1 to {MAX_LIMIT}"))
            }),
    }
}

/// `page` as a URL carries it: a whole number from 1 to [`MAX_PAGE`], and the
/// first when empty.
fn parse_page(text: &str) -> Result<u32, TelmoniError> {
    match text.trim() {
        "" => Ok(1),
        given => given
            .parse::<u32>()
            .ok()
            .filter(|page| (1..=MAX_PAGE).contains(page))
            .ok_or_else(|| {
                TelmoniError::BadRequest(format!("page is a whole number from 1 to {MAX_PAGE}"))
            }),
    }
}

/// The furthest west and east a clock is set from UTC, in minutes: UTC−12
/// and UTC+14.
const UTC_OFFSET_RANGE: std::ops::RangeInclusive<i16> = -720..=840;

/// `utc_offset` as a URL carries it: the reader's clock in whole minutes
/// east of UTC, and UTC itself when empty.
fn parse_utc_offset(text: &str) -> Result<i16, TelmoniError> {
    match text.trim() {
        "" => Ok(0),
        given => given
            .parse::<i16>()
            .ok()
            .filter(|minutes| UTC_OFFSET_RANGE.contains(minutes))
            .ok_or_else(|| {
                TelmoniError::BadRequest(
                    "utc_offset is whole minutes east of UTC, from -720 to 840".into(),
                )
            }),
    }
}

/// The two shapes a download comes in: CSV, for a spreadsheet, and JSON, the
/// list's own rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Csv,
    Json,
}

/// `format` as a URL carries it: `csv` when empty.
fn parse_format(text: &str) -> Result<Format, TelmoniError> {
    match text.trim() {
        "" | "csv" => Ok(Format::Csv),
        "json" => Ok(Format::Json),
        _ => Err(TelmoniError::BadRequest("format is csv or json".into())),
    }
}

/// What the list answers.
#[derive(Debug, Serialize)]
pub struct TraceList {
    /// The page's traces, in the order asked.
    pub traces: Vec<Trace>,
    /// How many days back the list read.
    pub days: u16,
    /// The most a page holds.
    pub limit: u32,
    /// Which page this is, from 1.
    pub page: u32,
    /// How many traces the filter admits in the window, on every page.
    pub total: u64,
    /// Whether a page after this one holds more.
    pub more: bool,
    /// The sort it is in.
    pub sort: TraceSort,
    /// And its direction.
    pub dir: SortDirection,
}

/// One trace of the list: a run's spans, summed ([`TraceRow`]).
#[derive(Debug, Serialize)]
pub struct Trace {
    pub run_id: Uuid,
    pub agent: String,
    pub user_id: String,
    pub session_id: String,
    /// Whether the run's own span has arrived.
    pub ended: bool,
    pub status: SpanStatus,
    /// The class of the error the run's own span ended on, never its
    /// message; empty otherwise.
    pub error_type: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_ms: i64,
    pub spans: u64,
    pub steps: u64,
    pub model_calls: u64,
    pub tool_calls: u64,
    pub errors: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub models: Vec<String>,
    pub tools: Vec<String>,
    /// The deployments its spans were sent from, each once: one, unless the
    /// run crossed services deployed apart.
    pub environments: Vec<String>,
    /// The releases its spans were sent from, each once.
    pub releases: Vec<String>,
}

impl From<TraceRow> for Trace {
    fn from(row: TraceRow) -> Self {
        Self {
            run_id: row.run_id,
            agent: row.agent,
            user_id: row.user_id,
            session_id: row.session_id,
            ended: row.ended != 0,
            status: row.status,
            error_type: row.error_type,
            started_at: row.started_at,
            ended_at: row.ended_at,
            duration_ms: row.duration_ms,
            spans: row.spans,
            steps: row.steps,
            model_calls: row.model_calls,
            tool_calls: row.tool_calls,
            errors: row.errors,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_read_input_tokens: row.cache_read_input_tokens,
            cache_write_input_tokens: row.cache_write_input_tokens,
            models: row.models,
            tools: row.tools,
            environments: row.environments,
            releases: row.releases,
        }
    }
}

/// A download's header row: the list's fields, in its order.
const CSV_HEADER: [&str; 23] = [
    "run_id",
    "agent",
    "user_id",
    "session_id",
    "ended",
    "status",
    "error_type",
    "started_at",
    "ended_at",
    "duration_ms",
    "spans",
    "steps",
    "model_calls",
    "tool_calls",
    "errors",
    "input_tokens",
    "output_tokens",
    "cache_read_input_tokens",
    "cache_write_input_tokens",
    "models",
    "tools",
    "environments",
    "releases",
];

impl Trace {
    /// The row a download writes for it, under [`CSV_HEADER`]: its times as
    /// RFC 3339, and its models, tools, environments and releases each one
    /// cell, `; ` between them.
    fn cells(&self) -> [String; 23] {
        let time = |t: &DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Micros, true);
        [
            self.run_id.to_string(),
            self.agent.clone(),
            self.user_id.clone(),
            self.session_id.clone(),
            self.ended.to_string(),
            self.status.to_string(),
            self.error_type.clone(),
            time(&self.started_at),
            time(&self.ended_at),
            self.duration_ms.to_string(),
            self.spans.to_string(),
            self.steps.to_string(),
            self.model_calls.to_string(),
            self.tool_calls.to_string(),
            self.errors.to_string(),
            self.input_tokens.to_string(),
            self.output_tokens.to_string(),
            self.cache_read_input_tokens.to_string(),
            self.cache_write_input_tokens.to_string(),
            self.models.join("; "),
            self.tools.join("; "),
            self.environments.join("; "),
            self.releases.join("; "),
        ]
    }
}

/// What the facets answer: each filter's values among the window's runs,
/// each counted under every other filter.
#[derive(Debug, Default, Serialize)]
pub struct TraceFacets {
    /// How many days back they read.
    pub days: u16,
    pub agent: Facet,
    pub user: Facet,
    pub session: Facet,
    pub status: Facet,
    pub model: Facet,
    pub tool: Facet,
    pub environment: Facet,
    pub release: Facet,
    /// Each tag as `key:value`.
    pub tag: Facet,
    pub prompt: Facet,
}

/// One filter's values: the ticked ones first, then the most runs, up to
/// [`FACET_VALUES_MAX`].
#[derive(Debug, Default, Serialize)]
pub struct Facet {
    pub values: Vec<FacetValue>,
    /// Whether the window holds values past these.
    pub more: bool,
}

/// One value, and how many runs carry it.
#[derive(Debug, Serialize)]
pub struct FacetValue {
    pub value: String,
    pub runs: u64,
}

impl TraceFacets {
    /// The read's rows, by facet, in the read's order.
    fn of(days: u16, rows: Vec<FacetRow>) -> Self {
        let mut facets = Self {
            days,
            ..Self::default()
        };
        for row in rows {
            let facet = match row.facet.as_str() {
                "agent" => &mut facets.agent,
                "user" => &mut facets.user,
                "session" => &mut facets.session,
                "status" => &mut facets.status,
                "model" => &mut facets.model,
                "tool" => &mut facets.tool,
                "environment" => &mut facets.environment,
                "release" => &mut facets.release,
                "tag" => &mut facets.tag,
                "prompt" => &mut facets.prompt,
                _ => continue,
            };
            if facet.values.len() == FACET_VALUES_MAX {
                facet.more = true;
                continue;
            }
            facet.values.push(FacetValue {
                value: row.value,
                runs: row.runs,
            });
        }
        facets
    }
}

/// What the histogram answers: the window in bars of equal width, from the
/// one the window opens in to the one now is in, each starting on the
/// reader's clock.
#[derive(Debug, Serialize)]
pub struct TraceHistogram {
    pub days: u16,
    /// How wide each bar is.
    pub bucket_seconds: u32,
    /// The reader's clock the bars start on, in minutes east of UTC.
    pub utc_offset: i16,
    pub bars: Vec<HistogramBar>,
}

/// One bar: the runs that started in it, by how they ended, their tokens,
/// and the median and 95th percentile of their durations, none where no run
/// started.
#[derive(Debug, Serialize)]
pub struct HistogramBar {
    pub at: DateTime<Utc>,
    pub ok: u64,
    pub error: u64,
    pub cancelled: u64,
    pub tokens: u64,
    pub duration_p50_ms: Option<f64>,
    pub duration_p95_ms: Option<f64>,
}

impl HistogramBar {
    fn of(bar: Bar) -> Option<Self> {
        Some(Self {
            at: DateTime::from_timestamp_millis(bar.at_ms)?,
            ok: bar.ok,
            error: bar.error,
            cancelled: bar.cancelled,
            tokens: bar.tokens,
            duration_p50_ms: bar.duration_p50_ms,
            duration_p95_ms: bar.duration_p95_ms,
        })
    }
}

/// What one trace's lane answers.
#[derive(Debug, Serialize)]
pub struct TraceDetail {
    pub run_id: Uuid,
    /// Its spans in the order they started, the run's own among them.
    pub spans: Vec<TraceSpan>,
    /// Whether the run holds more spans than the page shows.
    pub truncated: bool,
}

/// One span as the trace page reads it: the row, its maps as maps and its
/// times as text.
#[derive(Debug, Serialize)]
pub struct TraceSpan {
    pub span_id: String,
    pub parent_span_id: String,
    pub trace_id: String,
    pub agent: String,
    pub kind: SpanKind,
    pub name: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub status: SpanStatus,
    pub error_type: String,
    pub model: String,
    pub provider: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub user_id: String,
    pub session_id: String,
    pub environment: String,
    pub release: String,
    pub tags: BTreeMap<String, String>,
    pub sample_rate: Option<f64>,
    pub attributes: BTreeMap<String, String>,
    pub units: u32,
    pub received_at: DateTime<Utc>,
}

impl From<SpanRow> for TraceSpan {
    fn from(row: SpanRow) -> Self {
        Self {
            span_id: row.span_id,
            parent_span_id: row.parent_span_id,
            trace_id: row.trace_id,
            agent: row.agent,
            kind: row.kind,
            name: row.name,
            started_at: row.started_at,
            ended_at: row.ended_at,
            status: row.status,
            error_type: row.error_type,
            model: row.model,
            provider: row.provider,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_read_input_tokens: row.cache_read_input_tokens,
            cache_write_input_tokens: row.cache_write_input_tokens,
            user_id: row.user_id,
            session_id: row.session_id,
            environment: row.environment,
            release: row.release,
            tags: row.tags.into_iter().collect(),
            sample_rate: row.sample_rate,
            attributes: row.attributes.into_iter().collect(),
            units: row.units,
            received_at: row.received_at,
        }
    }
}

/// The project a trace lane reads, once the caller may read its traces.
async fn readable(state: &AppState, headers: &HeaderMap) -> Result<Projects, TelmoniError> {
    let acting = state.auth.resolve(headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Trace)?
        .project_id
        .clone();
    Ok(Projects::one(project_id))
}

/// `GET /internal/v1/telemetry/traces` — a page of the project's traces in the
/// window, filtered and sorted as the query says, and how many the filter
/// admits: two reads at once, since a page past the last has no row to
/// carry the count.
pub async fn list_traces(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<TraceList>, TelmoniError> {
    let projects = readable(&state, &headers).await?;
    let query = TraceQuery::read(pairs, Reading::List)?;
    let filter = query.filter()?;
    let (sort, dir) = query.sort()?;
    let limit = parse_limit(query.limit.as_deref().unwrap_or_default())?;
    let page = parse_page(query.page.as_deref().unwrap_or_default())?;
    let skipped = u64::from(page - 1) * u64::from(limit);
    let (rows, total) = tokio::try_join!(
        traces::list(&state.store, &projects, &filter, sort, dir, limit, skipped),
        traces::count(&state.store, &projects, &filter),
    )
    .map_err(read_failed)?;
    let more = skipped.saturating_add(u64::try_from(rows.len()).unwrap_or(u64::MAX)) < total;
    Ok(Json(TraceList {
        traces: rows.into_iter().map(Trace::from).collect(),
        days: filter.days,
        limit,
        page,
        total,
        more,
        sort,
        dir,
    }))
}

/// `GET /internal/v1/telemetry/traces/facets` — each filter's values among the
/// project's traces in the window, and how many runs carry each under every
/// other filter.
pub async fn get_facets(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<TraceFacets>, TelmoniError> {
    let projects = readable(&state, &headers).await?;
    let filter = TraceQuery::read(pairs, Reading::Facets)?.filter()?;
    let rows = traces::facets(&state.store, &projects, &filter)
        .await
        .map_err(read_failed)?;
    Ok(Json(TraceFacets::of(filter.days, rows)))
}

/// `GET /internal/v1/telemetry/traces/histogram` — the project's traces the
/// filter admits, by the bar they started in, the bars starting on the
/// reader's clock.
pub async fn get_histogram(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<TraceHistogram>, TelmoniError> {
    let projects = readable(&state, &headers).await?;
    let query = TraceQuery::read(pairs, Reading::Histogram)?;
    let filter = query.filter()?;
    let utc_offset = parse_utc_offset(query.utc_offset.as_deref().unwrap_or_default())?;
    let bucket_seconds = traces::bucket_seconds(filter.days);
    let rows = traces::histogram(&state.store, &projects, &filter, bucket_seconds, utc_offset)
        .await
        .map_err(read_failed)?;
    let bars = traces::bars(
        rows,
        Utc::now().timestamp_millis(),
        filter.days,
        bucket_seconds,
        utc_offset,
    );
    Ok(Json(TraceHistogram {
        days: filter.days,
        bucket_seconds,
        utc_offset,
        bars: bars.into_iter().filter_map(HistogramBar::of).collect(),
    }))
}

/// `GET /internal/v1/telemetry/traces/export` — the project's traces the filter
/// admits, sorted as asked, as a file: up to [`EXPORT_MAX`] of them, or the
/// runs `run` names alone.
pub async fn export_traces(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Response, TelmoniError> {
    let projects = readable(&state, &headers).await?;
    let query = TraceQuery::read(pairs, Reading::Export)?;
    let filter = query.filter()?;
    let (sort, dir) = query.sort()?;
    let format = parse_format(query.format.as_deref().unwrap_or_default())?;
    let rows = traces::list(&state.store, &projects, &filter, sort, dir, EXPORT_MAX, 0)
        .await
        .map_err(read_failed)?;
    let traces: Vec<Trace> = rows.into_iter().map(Trace::from).collect();
    Ok(match format {
        Format::Json => {
            let body = serde_json::to_vec(&traces).map_err(|e| {
                TelmoniError::Internal(format!("a trace download could not be written: {e}"))
            })?;
            ([(header::CONTENT_TYPE, "application/json")], body).into_response()
        }
        Format::Csv => {
            let mut body = csv::BOM.as_bytes().to_vec();
            csv::row(&mut body, &CSV_HEADER);
            for trace in &traces {
                csv::row(&mut body, &trace.cells());
            }
            ([(header::CONTENT_TYPE, "text/csv; charset=utf-8")], body).into_response()
        }
    })
}

/// `GET /internal/v1/telemetry/traces/{run_id}` — one trace's spans, the run's
/// own among them, in the order they started; `404` for a run the project
/// does not hold, which is also what another project's run id gets.
pub async fn get_trace(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<TraceDetail>, TelmoniError> {
    let projects = readable(&state, &headers).await?;
    let run_id: Uuid = run_id
        .parse()
        .map_err(|_| TelmoniError::BadRequest("a trace is named by its run's id, a UUID".into()))?;
    let rows = traces::spans(
        &state.store,
        &projects,
        run_id,
        SPANS_MAX + 1,
        TRACE_UNITS_MAX,
    )
    .await
    .map_err(read_failed)?;
    if rows.is_empty() {
        return Err(AuthError::NotFound("no trace with that run id in this project".into()).into());
    }
    let shown = traces::shown(rows.iter().map(|row| row.units), SPANS_MAX, TRACE_UNITS_MAX);
    let truncated = shown < rows.len();
    Ok(Json(TraceDetail {
        run_id,
        spans: rows.into_iter().take(shown).map(TraceSpan::from).collect(),
        truncated,
    }))
}

/// ClickHouse did not answer a read: an outage the console says so about, or
/// a shape of ours it refused, which retrying will not mend. The users and
/// sessions lanes answer the same way.
pub(crate) fn read_failed(error: clickhouse::error::Error) -> TelmoniError {
    let kind = store::error_kind(&error);
    if kind == "ours" {
        tracing::error!(kind, "traces: ClickHouse refused a read");
        return TelmoniError::Internal("traces: ClickHouse refused a read".into());
    }
    tracing::warn!(kind, "traces: ClickHouse did not answer a read");
    TelemetryError::StoreUnavailable("the span store did not answer; try again".into()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(query: &[(&str, &str)]) -> Vec<(String, String)> {
        query
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn read(reading: Reading, query: &[(&str, &str)]) -> Result<TraceQuery, TelmoniError> {
        TraceQuery::read(pairs(query), reading)
    }

    #[test]
    fn an_empty_query_is_the_default_filter_newest_first() {
        let query = read(Reading::List, &[]).expect("the default");
        assert_eq!(query.filter().expect("a filter"), TraceFilter::default());
        assert_eq!(
            query.sort().expect("a sort"),
            (TraceSort::Started, SortDirection::Desc)
        );
    }

    #[test]
    fn a_filter_given_again_admits_each_value_trimmed_and_once() {
        let query = read(
            Reading::List,
            &[
                ("agent", " nightly-report "),
                ("agent", "support-bot"),
                ("agent", "nightly-report"),
                ("agent", ""),
                ("user", "9f86d081884c7d659a2feaa0c55ad015"),
                ("session", "thread-42"),
                ("status", "error"),
                ("status", "cancelled"),
                ("model", "claude-sonnet-5-5"),
                ("tool", "search_docs"),
                ("environment", "production"),
                ("release", " 2026.10.18 "),
                ("tag", "team:search"),
                ("tag", "team:a:b"),
                ("prompt", "triage-v2"),
                ("sort", "user"),
                ("days", "30"),
                ("limit", "50"),
                ("page", "3"),
            ],
        )
        .expect("a full query");
        let filter = query.filter().expect("a filter");
        assert_eq!(filter.agents, ["nightly-report", "support-bot"]);
        assert_eq!(filter.user_ids, ["9f86d081884c7d659a2feaa0c55ad015"]);
        assert_eq!(filter.session_ids, ["thread-42"]);
        assert_eq!(filter.statuses, [SpanStatus::Error, SpanStatus::Cancelled]);
        assert_eq!(filter.models, ["claude-sonnet-5-5"]);
        assert_eq!(filter.tools, ["search_docs"]);
        assert_eq!(filter.environments, ["production"]);
        assert_eq!(filter.releases, ["2026.10.18"]);
        assert_eq!(
            filter.tags,
            ["team:search", "team:a:b"],
            "a tag's value may hold a colon of its own"
        );
        assert_eq!(filter.prompts, ["triage-v2"]);
        assert!(filter.runs.is_empty());
        assert_eq!(filter.days, 30);
        assert_eq!(
            query.sort().expect("a sort"),
            (TraceSort::User, SortDirection::Asc)
        );
        assert_eq!(
            parse_limit(query.limit.as_deref().unwrap_or_default()).ok(),
            Some(50)
        );
        assert_eq!(
            parse_page(query.page.as_deref().unwrap_or_default()).ok(),
            Some(3)
        );
    }

    #[test]
    fn a_column_sorts_its_own_way_unless_told() {
        let query = read(Reading::List, &[("sort", "tokens"), ("dir", "asc")]).expect("a sort");
        assert_eq!(
            query.sort().expect("a sort"),
            (TraceSort::Tokens, SortDirection::Asc)
        );
        for sort in TraceSort::all() {
            let expected = match sort {
                TraceSort::Agent | TraceSort::User | TraceSort::Session | TraceSort::Status => {
                    SortDirection::Asc
                }
                _ => SortDirection::Desc,
            };
            assert_eq!(default_direction(sort), expected, "{sort}");
        }
    }

    /// Each lane takes the filters and its own parameters, and no other's:
    /// a page is the list's, a clock the histogram's, a format and chosen
    /// runs a download's.
    #[test]
    fn each_lane_takes_its_own_parameters_and_no_others() {
        for (reading, name) in [
            (Reading::Facets, "sort"),
            (Reading::Facets, "page"),
            (Reading::Histogram, "limit"),
            (Reading::Histogram, "format"),
            (Reading::List, "utc_offset"),
            (Reading::List, "format"),
            (Reading::List, "run"),
            (Reading::Export, "page"),
            (Reading::Export, "utc_offset"),
        ] {
            let refused = read(reading, &[(name, "1")]).expect_err(name);
            assert_eq!(
                refused.to_problem_details().status,
                400,
                "{reading:?} {name}"
            );
        }
        assert!(read(Reading::Histogram, &[("utc_offset", "-300")]).is_ok());
        let download = read(
            Reading::Export,
            &[
                ("format", "json"),
                ("run", "6f2b8e3c-5d1a-4b7e-9c0f-1a2b3c4d5e6f"),
            ],
        )
        .expect("a download's query");
        assert_eq!(download.filter().expect("a filter").runs.len(), 1);
        assert_eq!(
            parse_format(download.format.as_deref().unwrap_or_default()).ok(),
            Some(Format::Json)
        );
        assert_eq!(parse_format("").ok(), Some(Format::Csv));
    }

    /// ⚠ A value the parameter does not take is a 400 naming the parameter,
    /// never the value, which a person typed; nor is a name the lane does
    /// not know echoed.
    #[test]
    fn a_value_a_parameter_does_not_take_is_refused_without_being_echoed() {
        let too_many: Vec<(String, String)> = (0..=FILTER_VALUES_MAX)
            .map(|i| ("agent".to_owned(), format!("agent-{i}")))
            .collect();
        let refusals: Vec<TelmoniError> = [
            read(Reading::List, &[("status", "maybe<script>")])
                .and_then(|q| q.filter().map(|_| ())),
            read(Reading::List, &[("sort", "spend")]).and_then(|q| q.sort().map(|_| ())),
            read(Reading::List, &[("dir", "sideways")]).and_then(|q| q.sort().map(|_| ())),
            read(Reading::List, &[("days", "0")]).and_then(|q| q.filter().map(|_| ())),
            read(Reading::List, &[("days", "361")]).and_then(|q| q.filter().map(|_| ())),
            read(Reading::List, &[("days", "7.5")]).and_then(|q| q.filter().map(|_| ())),
            read(Reading::List, &[("sort", "agent"), ("sort", "user")]).map(|_| ()),
            read(Reading::List, &[("nonsense<script>", "1")]).map(|_| ()),
            read(Reading::Export, &[("run", "not-a-run<script>")])
                .and_then(|q| q.filter().map(|_| ())),
            read(Reading::Export, &[("format", "xlsx<script>")])
                .and_then(|q| parse_format(q.format.as_deref().unwrap_or_default()).map(|_| ())),
            read(Reading::Histogram, &[("utc_offset", "841")]).and_then(|q| {
                parse_utc_offset(q.utc_offset.as_deref().unwrap_or_default()).map(|_| ())
            }),
            read(Reading::List, &[("agent", &"a".repeat(AGENT_MAX + 1))])
                .and_then(|q| q.filter().map(|_| ())),
            read(Reading::List, &[("tool", &"t".repeat(NAME_MAX + 1))])
                .and_then(|q| q.filter().map(|_| ())),
            read(
                Reading::List,
                &[("session", &"s".repeat(SESSION_ID_MAX + 1))],
            )
            .and_then(|q| q.filter().map(|_| ())),
            read(
                Reading::List,
                &[("environment", &"e".repeat(ENVIRONMENT_MAX + 1))],
            )
            .and_then(|q| q.filter().map(|_| ())),
            read(Reading::List, &[("tag", "untagged<script>")])
                .and_then(|q| q.filter().map(|_| ())),
            read(Reading::List, &[("tag", ":nameless")]).and_then(|q| q.filter().map(|_| ())),
            read(
                Reading::List,
                &[("tag", &format!("{}:v", "k".repeat(TAG_KEY_MAX + 1)))],
            )
            .and_then(|q| q.filter().map(|_| ())),
            read(
                Reading::List,
                &[("tag", &format!("k:{}", "v".repeat(TAG_VALUE_MAX + 1)))],
            )
            .and_then(|q| q.filter().map(|_| ())),
            TraceQuery::read(too_many, Reading::List).and_then(|q| q.filter().map(|_| ())),
        ]
        .into_iter()
        .filter_map(Result::err)
        .collect();
        assert_eq!(refusals.len(), 20, "every query above is refused");
        for (filter, value) in [
            ("agent", "a".repeat(AGENT_MAX)),
            ("tool", "t".repeat(NAME_MAX)),
        ] {
            let query = read(Reading::List, &[(filter, value.as_str())]);
            assert!(
                query.and_then(|q| q.filter()).is_ok(),
                "{filter} at its cap"
            );
        }
        for refused in refusals {
            let problem = refused.to_problem_details();
            assert_eq!(problem.status, 400);
            let detail = problem.detail.unwrap_or_default();
            assert!(
                !detail.contains("script")
                    && !detail.contains("sideways")
                    && !detail.contains("spend")
                    && !detail.contains("nonsense")
                    && !detail.contains("nameless"),
                "the value was echoed: {detail}"
            );
        }
        for (limit, page) in [("0", ""), ("501", ""), ("", "0"), ("", "1001")] {
            assert!(parse_limit(limit).is_err() || parse_page(page).is_err());
        }
    }

    /// A download names up to a page's rows, more than a filter admits; one
    /// more is refused.
    #[test]
    fn a_download_names_at_most_a_pages_runs() {
        let runs: Vec<(String, String)> = (0..=RUNS_MAX)
            .map(|_| ("run".to_owned(), Uuid::new_v4().to_string()))
            .collect();
        let page = TraceQuery::read(runs[1..].to_vec(), Reading::Export).expect("read");
        assert_eq!(page.filter().expect("a page's runs").runs.len(), RUNS_MAX);
        let query = TraceQuery::read(runs, Reading::Export).expect("read");
        assert_eq!(
            query
                .filter()
                .expect_err("too many")
                .to_problem_details()
                .status,
            400
        );
    }

    /// The facets keep the read's order, and say when a facet has values
    /// past the most they name.
    #[test]
    fn the_facets_keep_the_reads_order_and_say_when_there_are_more() {
        let mut rows: Vec<FacetRow> = (0..=FACET_VALUES_MAX)
            .zip((0..100_u64).rev())
            .map(|(i, runs)| FacetRow {
                facet: "model".into(),
                value: format!("model-{i}"),
                runs,
            })
            .collect();
        rows.push(FacetRow {
            facet: "status".into(),
            value: "error".into(),
            runs: 3,
        });
        let facets = TraceFacets::of(7, rows);
        assert_eq!(facets.model.values.len(), FACET_VALUES_MAX);
        assert!(facets.model.more);
        assert_eq!(facets.model.values[0].value, "model-0");
        assert_eq!(facets.status.values.len(), 1);
        assert!(!facets.status.more);
        assert!(facets.agent.values.is_empty());
    }

    /// A download's row is the list's fields under the header's names, each
    /// cell as a spreadsheet should read it.
    #[test]
    fn a_downloads_row_is_the_lists_fields_under_the_headers_names() {
        let trace = Trace {
            run_id: Uuid::nil(),
            agent: "=cmd".into(),
            user_id: String::new(),
            session_id: "thread, 42".into(),
            ended: true,
            status: SpanStatus::Error,
            error_type: "RuntimeError".into(),
            started_at: DateTime::from_timestamp_millis(1_791_721_800_000).expect("a time"),
            ended_at: DateTime::from_timestamp_millis(1_791_721_801_500).expect("a time"),
            duration_ms: 1_500,
            spans: 4,
            steps: 1,
            model_calls: 2,
            tool_calls: 1,
            errors: 1,
            input_tokens: 10,
            output_tokens: 5,
            cache_read_input_tokens: 0,
            cache_write_input_tokens: 0,
            models: vec!["a".into(), "b".into()],
            tools: vec![],
            environments: vec!["production".into()],
            releases: vec![],
        };
        let mut out = Vec::new();
        csv::row(&mut out, &trace.cells());
        let line = String::from_utf8(out).expect("UTF-8");
        assert!(line.starts_with(
            "00000000-0000-0000-0000-000000000000,\"'=cmd\",,\"thread, 42\",true,error,RuntimeError,"
        ));
        assert!(line.contains("2026-10-11T12:30:00.000000Z,2026-10-11T12:30:01.500000Z,1500,"));
        assert!(line.ends_with(",a; b,,production,\r\n"));
        let json = serde_json::to_value(&trace).expect("JSON");
        let mut keys: Vec<&str> = json
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        let mut header = CSV_HEADER.to_vec();
        header.sort_unstable();
        keys.sort_unstable();
        assert_eq!(keys, header, "the header names the JSON's every field");
    }
}
