//! The console's trace lanes, under `Resource::Trace`, which every role on a
//! project holds: `GET /internal/v1/telemetry/traces`, the list — a page of rows,
//! a row per trace in a window of days, filtered and sorted as the URL says,
//! and how many rows there are; `/traces/facets`, each filter's values and
//! how many traces carry each; `/traces/histogram`, the traces by when they
//! started; `/traces/export`, the rows as a file; and `/traces/{trace_id}`,
//! one trace's spans in the order they started. Each reads the spans of the
//! project `x-project-id` names and no other: the store names that project on
//! every read, twice.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::json;

use telmoni_shared::acting::{Acting, ActingKey};
use telmoni_shared::audit::Actor;
use telmoni_shared::csv;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped};
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, ProjectId, SortDirection, SpanKind, SpanStatus, TelmoniError,
    TelmoniResourceKind, TraceSort, UserId,
};

use crate::AppState;
use crate::content;
use crate::contract::{
    AGENT_MAX, ATTRIBUTE_VALUE_MAX, ENVIRONMENT_MAX, MODEL_MAX, NAME_MAX, RELEASE_MAX,
    SESSION_ID_MAX, TAG_KEY_MAX, TAG_VALUE_MAX, USER_ID_MAX, UTC_OFFSET_RANGE,
};
use crate::erasure::{self, Erase};
use crate::handler::chain::{self, Change};
use crate::ingest::batch::SpanContent;
use crate::spend::{self, Unread};
use crate::store::{self, Projects, SpanRow};
use crate::traces::{
    self, Bar, DEFAULT_LIMIT, EXPORT_MAX, FACET_VALUES_MAX, FILTER_VALUES_MAX, Facet, FacetRow,
    ListSort, Listing, MAX_DAYS, MAX_LIMIT, MAX_PAGE, SPANS_MAX, TRACE_UNITS_MAX, TraceFilter,
    TraceRow, Traces,
};

/// The longest value each filter takes, in characters, by its name in a URL:
/// a tag's is its key, colon and value.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct FilterValueMax {
    pub agent: usize,
    pub user: usize,
    pub session: usize,
    pub model: usize,
    pub tool: usize,
    pub environment: usize,
    pub release: usize,
    pub tag: usize,
    pub prompt: usize,
    pub prompt_version: usize,
}

pub const FILTER_VALUE_MAX: FilterValueMax = FilterValueMax {
    agent: AGENT_MAX,
    user: USER_ID_MAX,
    session: SESSION_ID_MAX,
    model: MODEL_MAX,
    tool: NAME_MAX,
    environment: ENVIRONMENT_MAX,
    release: RELEASE_MAX,
    tag: TAG_KEY_MAX + 1 + TAG_VALUE_MAX,
    prompt: ATTRIBUTE_VALUE_MAX,
    prompt_version: ATTRIBUTE_VALUE_MAX + 1 + ATTRIBUTE_VALUE_MAX,
};

/// Which lane a query string is read for. Each takes the filters and the
/// window, and its own parameters beside them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// A list: its sort, and its page.
    List,
    /// The facets: the filters alone.
    Facets,
    /// The histogram: the reader's clock.
    Histogram,
    /// A download: its sort and its format.
    Export,
}

/// The trace list's query string, text until checked. A filter may repeat,
/// each value one more the list admits, an empty one admitting nothing; any
/// other parameter comes once at most, and one the lane does not take is a 400.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TraceQuery {
    /// The traces' agents.
    pub agent: Vec<String>,
    /// Their end users, as stored.
    pub user: Vec<String>,
    /// Their sessions, by id.
    pub session: Vec<String>,
    /// How they ended: `ok`, `error` or `cancelled`.
    pub status: Vec<String>,
    pub fault: Vec<String>,
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
    /// Prompts by name and version any of their spans carry, each
    /// `name:version`.
    pub prompt_version: Vec<String>,
    /// A score the traces carry, by its name, and the least and most of its value.
    pub score: Option<String>,
    pub score_min: Option<String>,
    pub score_max: Option<String>,
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
            if let Ok(facet) = name.parse::<Facet>() {
                query.filter_mut(facet).push(value);
                continue;
            }
            let once = match (name.as_str(), reading) {
                ("days", _) => &mut query.days,
                ("sort", List | Export) => &mut query.sort,
                ("dir", List | Export) => &mut query.dir,
                ("limit", List) => &mut query.limit,
                ("page", List) => &mut query.page,
                ("utc_offset", Histogram) => &mut query.utc_offset,
                ("format", Export) => &mut query.format,
                ("score", _) => &mut query.score,
                ("score_min", _) => &mut query.score_min,
                ("score_max", _) => &mut query.score_max,
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

    /// One filter's values, by its facet.
    fn filter_mut(&mut self, facet: Facet) -> &mut Vec<String> {
        match facet {
            Facet::Agent => &mut self.agent,
            Facet::User => &mut self.user,
            Facet::Session => &mut self.session,
            Facet::Status => &mut self.status,
            Facet::Fault => &mut self.fault,
            Facet::Model => &mut self.model,
            Facet::Tool => &mut self.tool,
            Facet::Environment => &mut self.environment,
            Facet::Release => &mut self.release,
            Facet::Tag => &mut self.tag,
            Facet::Prompt => &mut self.prompt,
            Facet::PromptVersion => &mut self.prompt_version,
        }
    }

    /// The score the traces must carry: its name, and its value's bounds.
    fn score_filter(&self) -> Result<Option<crate::traces::ScoreFilter>, TelmoniError> {
        let bound = |given: &Option<String>, what: &str| -> Result<Option<f64>, TelmoniError> {
            given
                .as_deref()
                .map(|v| {
                    v.parse::<f64>()
                        .ok()
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| {
                            TelmoniError::BadRequest(format!("{what} is a finite number"))
                        })
                })
                .transpose()
        };
        let (min, max) = (
            bound(&self.score_min, "score_min")?,
            bound(&self.score_max, "score_max")?,
        );
        let Some(name) = self
            .score
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        else {
            if min.is_some() || max.is_some() {
                return Err(TelmoniError::BadRequest(
                    "score_min and score_max bound a score named by score".into(),
                ));
            }
            return Ok(None);
        };
        if name.chars().count() > crate::scores::NAME_MAX {
            return Err(TelmoniError::BadRequest(format!(
                "a score's name is up to {} characters",
                crate::scores::NAME_MAX
            )));
        }
        Ok(Some(crate::traces::ScoreFilter {
            name: name.to_owned(),
            min,
            max,
        }))
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
        Ok(TraceFilter {
            agents: values("agent", &self.agent, FILTER_VALUE_MAX.agent)?,
            user_ids: values("user", &self.user, FILTER_VALUE_MAX.user)?,
            session_ids: values("session", &self.session, FILTER_VALUE_MAX.session)?,
            statuses,
            faults: values("fault", &self.fault, usize::MAX)?
                .iter()
                .map(|given| {
                    given
                        .parse()
                        .map_err(|_| TelmoniError::BadRequest("fault is not a known class".into()))
                })
                .collect::<Result<Vec<_>, _>>()?,
            models: values("model", &self.model, FILTER_VALUE_MAX.model)?,
            tools: values("tool", &self.tool, FILTER_VALUE_MAX.tool)?,
            environments: values(
                "environment",
                &self.environment,
                FILTER_VALUE_MAX.environment,
            )?,
            releases: values("release", &self.release, FILTER_VALUE_MAX.release)?,
            tags: tag_values(&self.tag)?,
            prompts: values("prompt", &self.prompt, FILTER_VALUE_MAX.prompt)?,
            prompt_versions: prompt_version_values(&self.prompt_version)?,
            days: parse_days(self.days.as_deref())?,
            prices: crate::prices::PriceBook::default(),
            score: self.score_filter()?,
        })
    }

    /// The sort the query asks for, each column its own way unless the
    /// query says which.
    ///
    /// # Errors
    ///
    /// A column or a direction there is none of, never echoed.
    pub fn sort(&self) -> Result<(ListSort<TraceSort>, SortDirection), TelmoniError> {
        self.sort_for::<Traces>()
    }

    /// The sort the query asks for among a list's columns: the list's own
    /// when it names none, each column its own way unless it says which.
    ///
    /// # Errors
    ///
    /// A column or a direction there is none of, the refusal naming every
    /// column the list has and never the value.
    pub(crate) fn sort_for<L: Listing>(
        &self,
    ) -> Result<(ListSort<L::Sort>, SortDirection), TelmoniError> {
        let sort = match given(self.sort.as_deref()) {
            None => ListSort::Column(L::FALLBACK),
            Some(given) => ListSort::parse(given)
                .filter(|sort| matches!(sort, ListSort::Column(_)) || tag_sorts::<L>())
                .ok_or_else(sorts_refused::<L>)?,
        };
        let direction = match given(self.dir.as_deref()) {
            None => match &sort {
                ListSort::Column(column) => L::first_direction(*column),
                ListSort::Tag(_) => SortDirection::Asc,
            },
            Some(given) => given
                .parse::<SortDirection>()
                .map_err(|_| TelmoniError::BadRequest("dir is asc or desc".into()))?,
        };
        Ok((sort, direction))
    }

    /// The page the query asks for.
    ///
    /// # Errors
    ///
    /// A limit or a page out of range, never echoed.
    pub(crate) fn paging(&self) -> Result<Paging, TelmoniError> {
        let limit = parse_limit(self.limit.as_deref())?;
        let page = parse_page(self.page.as_deref())?;
        Ok(Paging {
            limit,
            page,
            skipped: u64::from(page - 1) * u64::from(limit),
        })
    }
}

/// A list's page: how many rows it holds, which it is from 1, and how many
/// rows come before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Paging {
    pub(crate) limit: u32,
    pub(crate) page: u32,
    pub(crate) skipped: u64,
}

impl Paging {
    /// Whether a page after this one holds more of `total` rows, once this
    /// one holds `shown`.
    pub(crate) fn more(self, shown: usize, total: u64) -> bool {
        self.skipped
            .saturating_add(u64::try_from(shown).unwrap_or(u64::MAX))
            < total
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
/// the traces a download names are a page's rows, more than a filter admits.
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
pub(crate) fn tag_values(given: &[String]) -> Result<Vec<String>, TelmoniError> {
    let pairs = values("tag", given, FILTER_VALUE_MAX.tag)?;
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

/// A prompt version filter's values, each `name:version` as a trace carries a
/// prompt where one span names both: compared whole, so a version is never
/// matched under another prompt's name.
pub(crate) fn prompt_version_values(given: &[String]) -> Result<Vec<String>, TelmoniError> {
    let pairs = values("prompt_version", given, FILTER_VALUE_MAX.prompt_version)?;
    for pair in &pairs {
        let well_formed = pair
            .split_once(':')
            .is_some_and(|(name, version)| !name.is_empty() && !version.is_empty());
        if !well_formed {
            return Err(TelmoniError::BadRequest(
                "prompt_version is name:version, each part at least one character".into(),
            ));
        }
    }
    Ok(pairs)
}

/// Whether a tag sorts list `L`.
fn tag_sorts<L: Listing>() -> bool {
    L::tag_sql(SortDirection::Asc).is_some()
}

/// What a sort the list does not take is refused with: every column it has,
/// and a tag where one sorts it.
fn sorts_refused<L: Listing>() -> TelmoniError {
    let mut words: Vec<String> = L::SORTS.iter().map(ToString::to_string).collect();
    if tag_sorts::<L>() {
        words.push("tag:<key>".into());
    }
    TelmoniError::BadRequest(format!("sort is one of {}", words.join(", ")))
}

/// A parameter's text, trimmed; `None` when absent or blank, which every
/// reader below takes as its default.
fn given(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|given| !given.is_empty())
}

/// A window of whole days from 1 to [`MAX_DAYS`].
pub(crate) fn check_days(days: u16) -> Result<u16, TelmoniError> {
    (1..=MAX_DAYS)
        .contains(&days)
        .then_some(days)
        .ok_or_else(|| {
            TelmoniError::BadRequest(format!("days is a whole number from 1 to {MAX_DAYS}"))
        })
}

/// `days` as a URL carries it, the default when absent; every page over the
/// spans reads its window so.
pub(crate) fn parse_days(text: Option<&str>) -> Result<u16, TelmoniError> {
    match given(text) {
        None => Ok(traces::DEFAULT_DAYS),
        Some(given) => check_days(given.parse().unwrap_or(0)),
    }
}

/// A page of rows from 1 to [`MAX_LIMIT`].
pub(crate) fn check_limit(limit: u32) -> Result<u32, TelmoniError> {
    (1..=MAX_LIMIT)
        .contains(&limit)
        .then_some(limit)
        .ok_or_else(|| {
            TelmoniError::BadRequest(format!("limit is a whole number from 1 to {MAX_LIMIT}"))
        })
}

/// `limit` as a URL carries it, the default when absent.
pub(crate) fn parse_limit(text: Option<&str>) -> Result<u32, TelmoniError> {
    match given(text) {
        None => Ok(DEFAULT_LIMIT),
        Some(given) => check_limit(given.parse().unwrap_or(0)),
    }
}

/// `page` as a URL carries it: a whole number from 1 to [`MAX_PAGE`], and the
/// first when absent.
fn parse_page(text: Option<&str>) -> Result<u32, TelmoniError> {
    match given(text) {
        None => Ok(1),
        Some(given) => given
            .parse::<u32>()
            .ok()
            .filter(|page| (1..=MAX_PAGE).contains(page))
            .ok_or_else(|| {
                TelmoniError::BadRequest(format!("page is a whole number from 1 to {MAX_PAGE}"))
            }),
    }
}

/// `utc_offset` as a URL carries it: the reader's clock in whole minutes
/// east of UTC, and UTC itself when absent.
fn parse_utc_offset(text: Option<&str>) -> Result<i16, TelmoniError> {
    match given(text) {
        None => Ok(0),
        Some(given) => given
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
pub(crate) enum Format {
    Csv,
    Json,
}

/// `format` as a URL carries it: `csv` when empty.
pub(crate) fn parse_format(text: Option<&str>) -> Result<Format, TelmoniError> {
    match given(text) {
        None | Some("csv") => Ok(Format::Csv),
        Some("json") => Ok(Format::Json),
        _ => Err(TelmoniError::BadRequest("format is csv or json".into())),
    }
}

/// A list's rows as a file: JSON, the list's own rows, or CSV under
/// `columns`, each row's cells as `cells` writes them. `what` names the
/// list in the error a row that cannot be written gives.
pub(crate) fn download<T: Serialize, const N: usize>(
    format: Format,
    rows: &[T],
    columns: &[&str; N],
    cells: impl Fn(&T) -> [String; N],
    what: &str,
) -> Result<Response, TelmoniError> {
    Ok(match format {
        Format::Json => {
            let body = serde_json::to_vec(rows).map_err(|e| {
                TelmoniError::internal(format!("a {what} download could not be written"), e)
            })?;
            ([(header::CONTENT_TYPE, "application/json")], body).into_response()
        }
        Format::Csv => {
            let mut body = csv::BOM.as_bytes().to_vec();
            csv::row(&mut body, columns);
            for row in rows {
                csv::row(&mut body, &cells(row));
            }
            ([(header::CONTENT_TYPE, "text/csv; charset=utf-8")], body).into_response()
        }
    })
}

/// What the list answers.
#[derive(Debug, Serialize)]
pub struct TraceList {
    /// The page's traces, in the order asked.
    pub traces: Vec<TraceRow>,
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
    pub sort: ListSort<TraceSort>,
    /// And its direction.
    pub dir: SortDirection,
}

/// A download's header row: the list's fields, in its order.
const CSV_HEADER: [&str; 28] = [
    "trace_id",
    "agent",
    "user_id",
    "session_id",
    "ended",
    "status",
    "error_type",
    "started_at",
    "ended_at",
    "duration_ms",
    "time_to_first_token_ms",
    "spans",
    "steps",
    "model_calls",
    "tool_calls",
    "retrievals",
    "errors",
    "input_tokens",
    "output_tokens",
    "cache_read_input_tokens",
    "cache_write_input_tokens",
    "reasoning_output_tokens",
    "spend",
    "models",
    "tools",
    "environments",
    "releases",
    "tags",
];

/// A download's columns: the list's, then the trace's content.
const DOWNLOAD_COLUMNS: usize = CSV_HEADER.len() + 1;

impl TraceRow {
    /// The row a download writes for it, under [`CSV_HEADER`]: its times as
    /// RFC 3339, and its models, tools, environments, releases and tags each
    /// one cell, `; ` between them.
    fn cells(&self) -> [String; 28] {
        let time = |t: &DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Micros, true);
        [
            self.trace_id.clone(),
            self.agent.clone(),
            self.user_id.clone(),
            self.session_id.clone(),
            self.ended.to_string(),
            self.status.to_string(),
            self.error_type.clone(),
            time(&self.started_at),
            time(&self.ended_at),
            self.duration_ms.to_string(),
            self.time_to_first_token_ms
                .map(|ms| ms.to_string())
                .unwrap_or_default(),
            self.spans.to_string(),
            self.steps.to_string(),
            self.model_calls.to_string(),
            self.tool_calls.to_string(),
            self.retrievals.to_string(),
            self.errors.to_string(),
            self.input_tokens.to_string(),
            self.output_tokens.to_string(),
            self.cache_read_input_tokens.to_string(),
            self.cache_write_input_tokens.to_string(),
            self.reasoning_output_tokens.to_string(),
            self.spend.to_string(),
            self.models.join("; "),
            self.tools.join("; "),
            self.environments.join("; "),
            self.releases.join("; "),
            self.tags.join("; "),
        ]
    }
}

/// What a list's facets answer: each filter's values among the window's
/// traces, each counted in the list's own rows under every other filter.
#[derive(Debug, Default, Serialize)]
pub struct Facets {
    /// How many days back they read.
    pub days: u16,
    pub agent: FacetValues,
    pub user: FacetValues,
    pub session: FacetValues,
    pub status: FacetValues,
    pub fault: FacetValues,
    pub model: FacetValues,
    pub tool: FacetValues,
    pub environment: FacetValues,
    pub release: FacetValues,
    /// Each tag as `key:value`.
    pub tag: FacetValues,
    pub prompt: FacetValues,
    /// Each prompt as `name:version`.
    pub prompt_version: FacetValues,
}

/// One filter's values: the ticked ones first, then the most rows, up to
/// [`FACET_VALUES_MAX`].
#[derive(Debug, Default, Serialize)]
pub struct FacetValues {
    pub values: Vec<FacetValue>,
    /// Whether the window holds values past these.
    pub more: bool,
}

/// One value, and how many of the list's rows carry it.
#[derive(Debug, Serialize)]
pub struct FacetValue {
    pub value: String,
    pub count: u64,
}

impl Facets {
    /// One facet's values.
    fn values_of(&mut self, facet: Facet) -> &mut FacetValues {
        match facet {
            Facet::Agent => &mut self.agent,
            Facet::User => &mut self.user,
            Facet::Session => &mut self.session,
            Facet::Status => &mut self.status,
            Facet::Fault => &mut self.fault,
            Facet::Model => &mut self.model,
            Facet::Tool => &mut self.tool,
            Facet::Environment => &mut self.environment,
            Facet::Release => &mut self.release,
            Facet::Tag => &mut self.tag,
            Facet::Prompt => &mut self.prompt,
            Facet::PromptVersion => &mut self.prompt_version,
        }
    }

    /// The read's rows, by facet, in the read's order.
    pub(crate) fn of(days: u16, rows: Vec<FacetRow>) -> Self {
        let mut facets = Self {
            days,
            ..Self::default()
        };
        for row in rows {
            let facet = facets.values_of(row.facet);
            if facet.values.len() == FACET_VALUES_MAX {
                facet.more = true;
                continue;
            }
            facet.values.push(FacetValue {
                value: row.value,
                count: row.listed,
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
    /// The score the list is narrowed to, whose mean each bar holds.
    pub score: Option<String>,
    pub bars: Vec<Bar>,
}

/// What a trace's deletion answers: how much of it went.
#[derive(Debug, Serialize)]
pub struct TraceDeleted {
    /// The trace deleted.
    pub trace_id: String,
    /// Its spans, marked deleted.
    pub spans: u64,
    /// The content fields its spans kept, deleted.
    pub fields: u64,
}

/// What one trace's lane answers.
#[derive(Debug, Serialize)]
pub struct TraceDetail {
    pub trace_id: String,
    /// Its spans in the order they started, the agent's own among them.
    pub spans: Vec<TraceSpan>,
    /// Whether the trace holds more spans than the page shows.
    pub truncated: bool,
    /// What the trace has spent, as an alert on its cost reads it; none
    /// where that could not be read.
    pub spend: Option<TraceSpend>,
    /// Each span's content, opened for the reader and filed as their read:
    /// empty where the trace kept none, none where it was not asked for or
    /// could not be opened.
    pub content: Option<Vec<SpanFields>>,
}

/// What a trace's model calls cost so far: each call's cost as reported,
/// else its tokens at Telmoni's price ([`spend::trace`]).
#[derive(Debug, Serialize)]
pub struct TraceSpend {
    /// In US dollars.
    pub spent: f64,
    /// The models its calls named that neither reported a cost nor have a
    /// price, which `spent` leaves out.
    pub unpriced: Vec<UnpricedModel>,
}

/// A model no price covers, by its provider.
#[derive(Debug, Serialize)]
pub struct UnpricedModel {
    pub provider: String,
    pub model: String,
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
    pub fault: Option<telmoni_shared::SpanFault>,
    pub model: String,
    pub provider: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub reasoning_output_tokens: u64,
    /// What it cost in US dollars, as reported; none where it reported none.
    pub cost: Option<f64>,
    /// Seconds to its answer's first token; none where it reported none.
    pub time_to_first_token: Option<f64>,
    /// Output tokens per second after the first token; none without valid timing.
    pub output_speed: Option<f64>,
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
        let output_speed = traces::output_speed(
            row.kind,
            row.output_tokens,
            row.ended_at - row.started_at,
            row.time_to_first_token,
        );
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
            fault: row.fault,
            model: row.model,
            provider: row.provider,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_read_input_tokens: row.cache_read_input_tokens,
            cache_write_input_tokens: row.cache_write_input_tokens,
            reasoning_output_tokens: row.reasoning_output_tokens,
            cost: row.cost,
            time_to_first_token: row.time_to_first_token,
            output_speed,
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

/// Who reads a project's traces, and the project.
pub(crate) struct Reader {
    pub(crate) project_id: ProjectId,
    pub(crate) by: ReadBy,
}

/// A person in the console, or a project API key on `/v1`.
pub(crate) enum ReadBy {
    User(UserId),
    Key(String),
}

impl Reader {
    /// A key's read of its own project, as `/v1` makes one.
    pub(crate) fn key(key: &ActingKey) -> Self {
        Self {
            project_id: key.project_id.clone(),
            by: ReadBy::Key(key.token_id.to_string()),
        }
    }

    /// Who a read of content is filed as.
    pub(crate) fn actor(&self) -> Actor<'_> {
        match &self.by {
            ReadBy::User(user_id) => Actor::User(user_id.as_str()),
            ReadBy::Key(token_id) => Actor::Key(token_id),
        }
    }
}

/// The caller, once they may read the project's traces.
pub(crate) async fn reader(state: &AppState, headers: &HeaderMap) -> Result<Reader, TelmoniError> {
    let acting = state.auth.resolve(headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Trace)?
        .project_id
        .clone();
    Ok(Reader {
        project_id,
        by: ReadBy::User(acting.user_id),
    })
}

/// The project a list lane reads, once the caller may read its traces.
pub(crate) async fn reading(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<ProjectId, TelmoniError> {
    Ok(reader(state, headers).await?.project_id)
}

/// `GET /internal/v1/telemetry/media/{sha256}` — a file the project keeps,
/// its read filed as a field's is: its bytes under its own type, served so a
/// browser runs nothing it holds; `404` where the project keeps no such file.
pub async fn get_media(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(sha256): Path<String>,
) -> Result<Response, TelmoniError> {
    let reader = reader(&state, &headers).await?;
    if !content::is_file_hash(&sha256) {
        return Err(TelmoniError::BadRequest(
            "a file is named by the SHA-256 of its bytes".into(),
        ));
    }
    let Some((mime_type, body)) =
        content::open_media(&state, reader.actor(), &reader.project_id, &sha256).await?
    else {
        return Err(AuthError::NotFound("the project keeps no such file".into()).into());
    };
    Ok((
        [
            (header::CONTENT_TYPE, mime_type),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (header::CONTENT_SECURITY_POLICY, "sandbox".to_owned()),
            (header::CACHE_CONTROL, "private, no-store".to_owned()),
        ],
        body,
    )
        .into_response())
}

/// One span's content as a lane answers it: each field kept, by its name.
#[derive(Debug, Serialize)]
pub struct SpanFields {
    pub trace_id: String,
    pub span_id: String,
    pub fields: BTreeMap<String, String>,
}

impl From<SpanContent> for SpanFields {
    fn from(span: SpanContent) -> Self {
        Self {
            trace_id: span.trace_id,
            span_id: span.span_id,
            fields: span.fields.into_iter().collect(),
        }
    }
}

/// `given` as a trace's id, 32 hex digits and never all zeros, lowercased;
/// else the caller's 400.
///
/// # Errors
///
/// A 400 naming the shape.
pub(crate) fn trace_id_of(given: &str) -> Result<String, TelmoniError> {
    let id = given.trim().to_ascii_lowercase();
    if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) && id.bytes().any(|b| b != b'0')
    {
        Ok(id)
    } else {
        Err(TelmoniError::BadRequest(
            "a trace is named by its id, 32 hex digits".into(),
        ))
    }
}

/// The traces' content, opened for the reader and filed as their read; none
/// where it could not be, and the page shows without it.
async fn content_of(
    state: &AppState,
    reader: &Reader,
    traces: &[String],
) -> Option<Vec<SpanFields>> {
    match content::open(state, reader.actor(), &reader.project_id, traces).await {
        Ok(opened) => Some(opened.into_iter().map(SpanFields::from).collect()),
        Err(error) => {
            tracing::warn!(
                project_id = %reader.project_id,
                error = %error,
                "traces: content could not be opened; the page shows without it"
            );
            None
        }
    }
}

/// The prices each call of the window is priced at; none where they could not
/// be read, so spend shows what calls reported alone.
pub(crate) async fn window_prices(
    state: &AppState,
    project_id: &ProjectId,
    days: u16,
) -> crate::prices::PriceBook {
    crate::spend::book_for_days(state, &Projects::one(project_id.clone()), days)
        .await
        .unwrap_or_else(|unread| {
            let kind = match &unread {
                crate::spend::Unread::Store(error) => crate::store::Fault::of(error).as_str(),
                crate::spend::Unread::Db(_) => "postgres",
            };
            tracing::warn!(
                project_id = %project_id,
                kind,
                "traces: the window's prices could not be read; spend shows what was reported"
            );
            crate::prices::PriceBook::default()
        })
}

/// The filter `query` asks for, priced where its rows show spend.
async fn filter_of(
    state: &AppState,
    project_id: &ProjectId,
    query: &TraceQuery,
    priced: bool,
) -> Result<TraceFilter, TelmoniError> {
    let mut filter = query.filter()?;
    filter.days = crate::capacity::visible_days(state, project_id, filter.days).await;
    if priced {
        filter.prices = window_prices(state, project_id, filter.days).await;
    }
    Ok(filter)
}

/// A list's page as its lane read it: the rows and the total, read at once
/// since a page past the last has no row to carry the count, and what the
/// query asked.
pub(crate) struct Listed<L: Listing> {
    pub(crate) rows: Vec<L::Row>,
    pub(crate) total: u64,
    pub(crate) paging: Paging,
    pub(crate) sort: ListSort<L::Sort>,
    pub(crate) dir: SortDirection,
    pub(crate) days: u16,
}

impl<L: Listing> Listed<L> {
    /// Whether a page after this one holds more.
    pub(crate) fn more(&self) -> bool {
        self.paging.more(self.rows.len(), self.total)
    }
}

/// A page of one list of the project's traces in the window, filtered and
/// sorted as the query says, with how many the filter admits.
pub(crate) async fn list_page<L: Listing>(
    state: &AppState,
    project_id: &ProjectId,
    pairs: Vec<(String, String)>,
) -> Result<Listed<L>, TelmoniError> {
    let projects = Projects::one(project_id.clone());
    let query = TraceQuery::read(pairs, Reading::List)?;
    let filter = filter_of(state, project_id, &query, true).await?;
    let (sort, dir) = query.sort_for::<L>()?;
    let paging = query.paging()?;
    let (rows, total) = tokio::try_join!(
        traces::list::<L>(
            &state.store,
            &projects,
            &filter,
            (&sort, dir),
            paging.limit,
            paging.skipped
        ),
        traces::count::<L>(&state.store, &projects, &filter),
    )
    .map_err(read_failed)?;
    Ok(Listed {
        rows,
        total,
        paging,
        sort,
        dir,
        days: filter.days,
    })
}

/// One list's facets: each filter's values among the project's traces in the
/// window, and how many of the list's rows carry each under every other
/// filter.
pub(crate) async fn facets_page<L: Listing>(
    state: &AppState,
    headers: &HeaderMap,
    pairs: Vec<(String, String)>,
) -> Result<Facets, TelmoniError> {
    let project_id = reading(state, headers).await?;
    let projects = Projects::one(project_id.clone());
    let query = TraceQuery::read(pairs, Reading::Facets)?;
    let filter = filter_of(state, &project_id, &query, false).await?;
    let rows = traces::facets::<L>(&state.store, &projects, &filter)
        .await
        .map_err(read_failed)?;
    Ok(Facets::of(filter.days, rows))
}

/// One list's rows for a download, sorted as asked, up to [`EXPORT_MAX`] of
/// them, and the format asked for.
pub(crate) async fn export_rows<L: Listing>(
    state: &AppState,
    headers: &HeaderMap,
    pairs: Vec<(String, String)>,
    reading: Reading,
) -> Result<(Vec<L::Row>, Format, Reader), TelmoniError> {
    let reader = reader(state, headers).await?;
    let projects = Projects::one(reader.project_id.clone());
    let query = TraceQuery::read(pairs, reading)?;
    let filter = filter_of(state, &reader.project_id, &query, true).await?;
    let (sort, dir) = query.sort_for::<L>()?;
    let format = parse_format(query.format.as_deref())?;
    let rows = traces::list::<L>(
        &state.store,
        &projects,
        &filter,
        (&sort, dir),
        EXPORT_MAX,
        0,
    )
    .await
    .map_err(read_failed)?;
    Ok((rows, format, reader))
}

/// `GET /internal/v1/telemetry/traces` — a page of the project's traces in the
/// window, filtered and sorted as the query says, and how many the filter
/// admits.
pub async fn list_traces(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<ScoredTraceList>, TelmoniError> {
    let project_id = reading(&state, &headers).await?;
    let list = trace_list(&state, &project_id, pairs).await?;
    let traces: Vec<String> = list.traces.iter().map(|t| t.trace_id.clone()).collect();
    let scores = crate::scores::of_traces(&state.store, &Projects::one(project_id), &traces)
        .await
        .map_err(read_failed)?;
    Ok(Json(ScoredTraceList { list, scores }))
}

/// The console's page of traces, and each trace's own scores by name.
#[derive(Debug, Serialize)]
pub struct ScoredTraceList {
    #[serde(flatten)]
    pub list: TraceList,
    pub scores: std::collections::BTreeMap<String, Vec<crate::scores::Summary>>,
}

/// A page of the project's traces, as the console's lane and `/v1` answer it.
pub(crate) async fn trace_list(
    state: &AppState,
    project_id: &ProjectId,
    pairs: Vec<(String, String)>,
) -> Result<TraceList, TelmoniError> {
    let listed = list_page::<Traces>(state, project_id, pairs).await?;
    Ok(TraceList {
        more: listed.more(),
        traces: listed.rows,
        days: listed.days,
        limit: listed.paging.limit,
        page: listed.paging.page,
        total: listed.total,
        sort: listed.sort,
        dir: listed.dir,
    })
}

/// `GET /internal/v1/telemetry/rollups` — the project's hourly totals over
/// the window, by the hour or the day, grouped where asked: what the
/// dashboards page draws, read under the trace list's role.
pub async fn get_rollups(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(asked): Query<crate::rollups::MetricsAsked>,
) -> Result<Json<crate::rollups::Metrics>, TelmoniError> {
    let project_id = reading(&state, &headers).await?;
    Ok(Json(
        crate::rollups::read(&state, &project_id, &asked).await?,
    ))
}

/// `GET /internal/v1/telemetry/traces/facets` — each filter's values among the
/// project's traces in the window, and how many carry each under every
/// other filter.
pub async fn get_facets(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<Facets>, TelmoniError> {
    Ok(Json(facets_page::<Traces>(&state, &headers, pairs).await?))
}

/// `GET /internal/v1/telemetry/traces/histogram` — the project's traces the
/// filter admits, by the bar they started in, the bars starting on the
/// reader's clock.
pub async fn get_histogram(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<TraceHistogram>, TelmoniError> {
    let project_id = reading(&state, &headers).await?;
    let projects = Projects::one(project_id.clone());
    let query = TraceQuery::read(pairs, Reading::Histogram)?;
    let filter = filter_of(&state, &project_id, &query, true).await?;
    let utc_offset = parse_utc_offset(query.utc_offset.as_deref())?;
    let bucket_seconds = traces::bucket_seconds(filter.days);
    let rows = traces::histogram(&state.store, &projects, &filter, bucket_seconds, utc_offset)
        .await
        .map_err(read_failed)?;
    let means = traces::score_means(&state.store, &projects, &filter, bucket_seconds, utc_offset)
        .await
        .map_err(read_failed)?;
    let mut bars = traces::bars(
        rows,
        Utc::now().timestamp_millis(),
        filter.days,
        bucket_seconds,
        utc_offset,
    );
    traces::with_score_means(&mut bars, &means);
    let tokens =
        traces::tokens_by_model(&state.store, &projects, &filter, bucket_seconds, utc_offset)
            .await
            .map_err(read_failed)?;
    traces::with_model_metrics(&mut bars, tokens);
    Ok(Json(TraceHistogram {
        days: filter.days,
        bucket_seconds,
        utc_offset,
        score: filter.score.as_ref().map(|score| score.name.clone()),
        bars,
    }))
}

/// `GET /internal/v1/telemetry/traces/export` — the project's traces the filter
/// admits, sorted as asked, as a file: up to [`EXPORT_MAX`] of them.
pub async fn export_traces(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Response, TelmoniError> {
    let (rows, format, reader) =
        export_rows::<Traces>(&state, &headers, pairs, Reading::Export).await?;
    // One read of every trace's content, filed once; a download that cannot
    // open it fails rather than leaving it out unsaid.
    let traces: Vec<String> = rows.iter().map(|row| row.trace_id.clone()).collect();
    let mut kept: HashMap<String, Vec<SpanFields>> = HashMap::new();
    for span in content::open(&state, reader.actor(), &reader.project_id, &traces).await? {
        kept.entry(span.trace_id.clone())
            .or_default()
            .push(span.into());
    }
    let rows: Vec<TraceDownload> = rows
        .into_iter()
        .map(|row| TraceDownload {
            content: kept.remove(&row.trace_id).unwrap_or_default(),
            row,
        })
        .collect();
    let header: [&str; DOWNLOAD_COLUMNS] =
        std::array::from_fn(|i| CSV_HEADER.get(i).copied().unwrap_or("content"));
    download(format, &rows, &header, TraceDownload::cells, "trace")
}

/// A trace as its download writes it: the list's row and each span's content,
/// empty where the trace kept none.
#[derive(Debug, Serialize)]
pub struct TraceDownload {
    #[serde(flatten)]
    row: TraceRow,
    content: Vec<SpanFields>,
}

impl TraceDownload {
    /// The row's cells and its content as one JSON cell.
    fn cells(&self) -> [String; DOWNLOAD_COLUMNS] {
        let row = self.row.cells();
        let content = serde_json::to_string(&self.content).unwrap_or_default();
        std::array::from_fn(|i| row.get(i).cloned().unwrap_or_else(|| content.clone()))
    }
}

/// `DELETE /internal/v1/telemetry/traces/{trace_id}` — one trace deleted by a
/// project admin: the request filed in the organization's chain first, then
/// the trace marked deleted in ClickHouse and its content deleted, and the
/// receipt mailed to whoever asked. `404` for a trace the project does not hold.
pub async fn delete_trace(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(trace_id): Path<String>,
) -> Result<Json<TraceDeleted>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Delete, Resource::Trace)?
        .project_id;
    let trace_id = trace_id_of(&trace_id)?;
    let found = erasure::find(&state, project_id, Erase::Trace(&trace_id)).await?;
    if found.traces.is_empty() {
        return Err(AuthError::NotFound("no trace with that id in this project".into()).into());
    }
    let spans = found.spans;
    chain::write(
        &state.db,
        &acting,
        project_id,
        "trace deletion",
        async |tx| file_trace_deletion(&state, &acting, project_id, tx, &trace_id, spans).await,
    )
    .await?;
    let erase = Erase::Trace(&trace_id);
    let fields = erasure::carry_out(&state, project_id, erase, found, &acting.user_id).await?;
    tracing::info!(project_id = %project_id, trace_id = %trace_id, spans, fields, "telemetry: a trace deleted");
    Ok(Json(TraceDeleted {
        trace_id,
        spans,
        fields,
    }))
}

/// The deletion's request, filed under the organization's chain lock, the
/// role checked again on auth's second answer.
async fn file_trace_deletion(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    trace_id: &str,
    spans: u64,
) -> Result<(), TelmoniError> {
    chain::held(
        state.auth.as_ref(),
        acting,
        tx,
        Verb::Delete,
        Resource::Trace,
    )
    .await?;
    let change = Change {
        action: AuditAction::Deleted,
        kind: TelmoniResourceKind::Trace,
        id: trace_id.to_owned(),
        metadata: json!({ "spans": spans }),
    };
    chain::file(tx, acting, project_id, change).await?;
    Ok(())
}

/// `GET /internal/v1/telemetry/traces/{trace_id}` — one trace's spans, the
/// agent's own among them, in the order they started, its content opened and
/// filed only where asked (`content=true`); `404` for a trace the project
/// does not hold.
pub async fn get_trace(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(trace_id): Path<String>,
    Query(asked): Query<super::v1::TraceAsked>,
) -> Result<Json<TraceDetail>, TelmoniError> {
    let reader = reader(&state, &headers).await?;
    Ok(Json(
        trace_detail(&state, &reader, &trace_id, asked.content).await?,
    ))
}

/// One trace as the console's lane and `/v1` answer it, its content opened
/// and filed as the reader's read where `with_content` asks for it.
pub(crate) async fn trace_detail(
    state: &AppState,
    reader: &Reader,
    trace_id: &str,
    with_content: bool,
) -> Result<TraceDetail, TelmoniError> {
    let projects = Projects::one(reader.project_id.clone());
    let trace_id = trace_id_of(trace_id)?;
    let rows = traces::spans(
        &state.store,
        &projects,
        &trace_id,
        SPANS_MAX + 1,
        TRACE_UNITS_MAX,
    )
    .await
    .map_err(read_failed)?;
    // A trace older than the organization's retention reaches is not there,
    // as the list would not name it: held, it is hidden until it goes.
    let reach = crate::capacity::visible_days(state, &reader.project_id, u16::MAX).await;
    let past = reach < u16::MAX
        && rows
            .iter()
            .all(|row| row.received_at < Utc::now() - chrono::Duration::days(i64::from(reach)));
    if rows.is_empty() || past {
        return Err(AuthError::NotFound("no trace with that id in this project".into()).into());
    }
    let shown = traces::shown(rows.iter().map(|row| row.units), SPANS_MAX, TRACE_UNITS_MAX);
    let truncated = shown < rows.len();
    let content = if with_content {
        content_of(state, reader, std::slice::from_ref(&trace_id)).await
    } else {
        None
    };
    Ok(TraceDetail {
        spans: rows.into_iter().take(shown).map(TraceSpan::from).collect(),
        truncated,
        spend: spend_of(state, &projects, &trace_id).await,
        content,
        trace_id,
    })
}

/// What the trace has spent; none, logged by the error's kind, where a store
/// does not answer, and the trace shows without it.
async fn spend_of(state: &AppState, projects: &Projects, trace_id: &str) -> Option<TraceSpend> {
    match spend::trace(state, projects, trace_id).await {
        Ok(trace) => Some(TraceSpend {
            spent: trace.spent,
            unpriced: trace
                .unpriced
                .into_iter()
                .map(|(provider, model)| UnpricedModel { provider, model })
                .collect(),
        }),
        Err(Unread::Store(error)) => {
            tracing::warn!(
                trace_id = %trace_id,
                kind = store::Fault::of(&error).as_str(),
                "traces: what a trace spent could not be read; it shows without"
            );
            None
        }
        Err(Unread::Db(error)) => {
            tracing::warn!(
                trace_id = %trace_id,
                error = %error,
                "traces: the prices of what a trace spent could not be read; it shows without"
            );
            None
        }
    }
}

/// ClickHouse did not answer a read: an outage the console says so about, or
/// a shape of ours it refused, which retrying will not mend.
pub(crate) fn read_failed(error: clickhouse::error::Error) -> TelmoniError {
    store::failed(
        &error,
        "traces: a read",
        "the span store did not answer; try again",
    )
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
            (ListSort::Column(TraceSort::Started), SortDirection::Desc)
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
                ("prompt_version", "triage:3"),
                ("prompt_version", "triage:v:2"),
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
        assert_eq!(
            filter.prompt_versions,
            ["triage:3", "triage:v:2"],
            "a version may hold a colon of its own"
        );
        assert_eq!(filter.days, 30);
        assert_eq!(
            query.sort().expect("a sort"),
            (ListSort::Column(TraceSort::User), SortDirection::Asc)
        );
        assert_eq!(parse_limit(query.limit.as_deref()).ok(), Some(50));
        assert_eq!(parse_page(query.page.as_deref()).ok(), Some(3));
    }

    #[test]
    fn a_column_sorts_its_own_way_unless_told() {
        let query = read(Reading::List, &[("sort", "tokens"), ("dir", "asc")]).expect("a sort");
        assert_eq!(
            query.sort().expect("a sort"),
            (ListSort::Column(TraceSort::Tokens), SortDirection::Asc)
        );
        for sort in TraceSort::all() {
            let expected = match sort {
                TraceSort::Agent | TraceSort::User | TraceSort::Session | TraceSort::Status => {
                    SortDirection::Asc
                }
                _ => SortDirection::Desc,
            };
            assert_eq!(Traces::first_direction(sort), expected, "{sort}");
        }
    }

    /// Each lane takes the filters and its own parameters, and no other's:
    /// a page is a list's, a clock the histogram's, and a format a download's.
    #[test]
    fn each_lane_takes_its_own_parameters_and_no_others() {
        for (reading, name) in [
            (Reading::Facets, "sort"),
            (Reading::Facets, "page"),
            (Reading::Histogram, "limit"),
            (Reading::Histogram, "format"),
            (Reading::List, "utc_offset"),
            (Reading::List, "format"),
            (Reading::List, "trace"),
            (Reading::Export, "page"),
            (Reading::Export, "limit"),
            (Reading::Export, "utc_offset"),
            (Reading::Export, "trace"),
        ] {
            let refused = read(reading, &[(name, "1")]).expect_err(name);
            assert_eq!(
                refused.to_problem_details().status,
                400,
                "{reading:?} {name}"
            );
        }
        assert!(read(Reading::Histogram, &[("utc_offset", "-300")]).is_ok());
        let download = read(Reading::Export, &[("sort", "tokens"), ("format", "json")])
            .expect("a download's query");
        assert_eq!(
            parse_format(download.format.as_deref()).ok(),
            Some(Format::Json)
        );
        assert_eq!(parse_format(None).ok(), Some(Format::Csv));
    }

    /// ⚠ A value the parameter does not take is a 400 naming the parameter,
    /// never the value, which a person typed; nor is a name the lane does
    /// not know echoed.
    #[test]
    fn a_value_a_parameter_does_not_take_is_refused_without_being_echoed() {
        let too_many: Vec<(String, String)> = (0..=FILTER_VALUES_MAX)
            .map(|i| ("agent".to_owned(), format!("agent-{i}")))
            .collect();
        let answers = [
            read(Reading::List, &[("status", "maybe<script>")])
                .and_then(|q| q.filter().map(|_| ())),
            read(Reading::List, &[("sort", "price")]).and_then(|q| q.sort().map(|_| ())),
            read(Reading::List, &[("dir", "sideways")]).and_then(|q| q.sort().map(|_| ())),
            read(Reading::List, &[("days", "0")]).and_then(|q| q.filter().map(|_| ())),
            read(Reading::List, &[("days", "361")]).and_then(|q| q.filter().map(|_| ())),
            read(Reading::List, &[("days", "7.5")]).and_then(|q| q.filter().map(|_| ())),
            read(Reading::List, &[("sort", "agent"), ("sort", "user")]).map(|_| ()),
            read(Reading::List, &[("nonsense<script>", "1")]).map(|_| ()),
            read(Reading::Export, &[("format", "xlsx<script>")])
                .and_then(|q| parse_format(q.format.as_deref()).map(|_| ())),
            read(Reading::Histogram, &[("utc_offset", "841")])
                .and_then(|q| parse_utc_offset(q.utc_offset.as_deref()).map(|_| ())),
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
        ];
        let asked = answers.len();
        let refusals: Vec<TelmoniError> = answers.into_iter().filter_map(Result::err).collect();
        assert_eq!(refusals.len(), asked, "every query above is refused");
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
                    && !detail.contains("price")
                    && !detail.contains("nonsense")
                    && !detail.contains("nameless"),
                "the value was echoed: {detail}"
            );
        }
        for (limit, page) in [("0", ""), ("501", ""), ("", "0"), ("", "1001")] {
            assert!(parse_limit(Some(limit)).is_err() || parse_page(Some(page)).is_err());
        }
    }

    /// The facets keep the read's order, and say when a facet has values
    /// past the most they name.
    #[test]
    fn the_facets_keep_the_reads_order_and_say_when_there_are_more() {
        let mut rows: Vec<FacetRow> = (0..=FACET_VALUES_MAX)
            .zip((0..100_u64).rev())
            .map(|(i, listed)| FacetRow {
                facet: Facet::Model,
                value: format!("model-{i}"),
                listed,
            })
            .collect();
        rows.push(FacetRow {
            facet: Facet::Status,
            value: "error".into(),
            listed: 3,
        });
        let facets = Facets::of(7, rows);
        assert_eq!(facets.model.values.len(), FACET_VALUES_MAX);
        assert!(facets.model.more);
        assert_eq!(facets.model.values[0].value, "model-0");
        assert_eq!(facets.status.values[0].count, 3);
        assert_eq!(facets.status.values.len(), 1);
        assert!(!facets.status.more);
        assert!(facets.agent.values.is_empty());
    }

    /// A download's row is the list's fields under the header's names, each
    /// cell as a spreadsheet should read it.
    #[test]
    fn a_downloads_row_is_the_lists_fields_under_the_headers_names() {
        let trace = TraceRow {
            trace_id: "ab".repeat(16),
            agent: "=cmd".into(),
            user_id: String::new(),
            session_id: "thread, 42".into(),
            ended: true,
            status: SpanStatus::Error,
            error_type: "RuntimeError".into(),
            started_at: DateTime::from_timestamp_millis(1_791_721_800_000).expect("a time"),
            ended_at: DateTime::from_timestamp_millis(1_791_721_801_500).expect("a time"),
            duration_ms: 1_500,
            time_to_first_token_ms: Some(250.0),
            spans: 4,
            steps: 1,
            model_calls: 2,
            tool_calls: 1,
            retrievals: 3,
            errors: 1,
            input_tokens: 10,
            output_tokens: 5,
            cache_read_input_tokens: 0,
            cache_write_input_tokens: 0,
            reasoning_output_tokens: 0,
            spend: 0.25,
            models: vec!["a".into(), "b".into()],
            tools: vec![],
            environments: vec!["production".into()],
            releases: vec![],
            tags: vec!["repo:telmoni".into()],
        };
        let mut out = Vec::new();
        csv::row(&mut out, &trace.cells());
        let line = String::from_utf8(out).expect("UTF-8");
        assert!(line.starts_with(
            "abababababababababababababababab,\"'=cmd\",,\"thread, 42\",true,error,RuntimeError,"
        ));
        assert!(
            line.contains(
                "2026-10-11T12:30:00.000000Z,2026-10-11T12:30:01.500000Z,1500,250,4,1,2,1,3,1,10,5,"
            ),
            "each count under its own name"
        );
        assert!(line.ends_with(",a; b,,production,,repo:telmoni\r\n"));
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
