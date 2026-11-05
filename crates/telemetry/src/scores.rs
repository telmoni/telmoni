//! Scores: of a run, one of its spans or a session, each a name and a value,
//! kept in `telemetry.scores` beside the spans. They come from the
//! application's own code, as OpenTelemetry's `gen_ai.evaluation.result`
//! event on the span it scores, or `POST /v1/scores`; or from a member on a
//! trace's page ([`crate::handler::scores`]). Metadata alone: a score's
//! comment is content, kept only where its project keeps content
//! ([`crate::content::keep_comment`]).

use chrono::{DateTime, Utc};
use clickhouse::Row;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::trace::v1::Span;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::{ProjectId, ScoreSource, ScoreTarget, ScoreType, TelmoniError};

use crate::store::text;

/// OpenTelemetry's GenAI evaluation event, and the attributes read from it.
pub const EVALUATION_EVENT: &str = "gen_ai.evaluation.result";
pub const EVALUATION_NAME: &str = "gen_ai.evaluation.name";
pub const EVALUATION_VALUE: &str = "gen_ai.evaluation.score.value";
pub const EVALUATION_LABEL: &str = "gen_ai.evaluation.score.label";
/// Telmoni's own, beside OpenTelemetry's: `boolean` for a yes-or-no score,
/// a number's range, and the version of the code that scored.
pub const SCORE_TYPE: &str = "telmoni.score.type";
pub const SCORE_RANGE_MIN: &str = "telmoni.score.range_min";
pub const SCORE_RANGE_MAX: &str = "telmoni.score.range_max";
pub const SCORE_VERSION: &str = "telmoni.score.version";

/// The longest score name, and the most categories one score names, each
/// up to [`CATEGORY_MAX`] characters.
pub const NAME_MAX: usize = 64;
pub const CATEGORIES_MAX: usize = 10;
pub const CATEGORY_MAX: usize = 64;
/// The longest scorer's version the application names.
pub const VERSION_MAX: usize = 64;
/// A score's comment, content, as the content its project keeps holds it:
/// under the score's run, as a field of a span named by the score's id, which
/// no span's 16 hex digits can be, so it goes as the run's content goes.
pub const COMMENT_FIELD: &str = "telmoni.score.comment";
/// The longest comment, in characters.
pub const COMMENT_MAX: usize = 4_000;

/// What the span a score's comment is kept under begins with.
pub const COMMENT_SPAN_PREFIX: &str = "score/";

/// The span a score's comment is kept under, by the score's id.
#[must_use]
pub fn comment_span(score_id: Uuid) -> String {
    format!("{COMMENT_SPAN_PREFIX}{score_id}")
}

/// The name scores' ids are made under, so no other id collides.
const NAMESPACE: Uuid = Uuid::from_u128(0x2c7e_91a4_5b03_4f68_a1d2_7e40_93b5_c618);

/// One score, as `telemetry.scores` holds it.
#[derive(Debug, Clone, PartialEq, Row, Serialize, Deserialize)]
pub struct ScoreRow {
    pub project_id: ProjectId,
    #[serde(with = "clickhouse::serde::chrono::datetime64::micros")]
    pub received_at: DateTime<Utc>,
    #[serde(with = "clickhouse::serde::uuid")]
    pub score_id: Uuid,
    #[serde(with = "text")]
    pub target: ScoreTarget,
    /// Nil for a session's, and for another exporter's span until its trace is placed.
    #[serde(with = "clickhouse::serde::uuid")]
    pub run_id: Uuid,
    pub trace_id: String,
    pub span_id: String,
    pub session_id: String,
    pub name: String,
    #[serde(with = "text")]
    pub data_type: ScoreType,
    pub value: f64,
    pub categories: Vec<String>,
    pub range_min: Option<f64>,
    pub range_max: Option<f64>,
    #[serde(with = "text")]
    pub source: ScoreSource,
    /// The member's user id; empty for the application's.
    pub scorer: String,
    pub scorer_version: String,
    pub is_deleted: u8,
}

/// What a score says, before it names what it scores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Judgement {
    pub name: String,
    pub data_type: ScoreType,
    /// A number's value, or a yes-or-no's, 1 or 0; none for a category.
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub range_min: Option<f64>,
    #[serde(default)]
    pub range_max: Option<f64>,
}

impl Judgement {
    /// The judgement held to a score's rules, its name trimmed: a name of 1 to
    /// [`NAME_MAX`] characters, a finite number within its range, a yes or no
    /// of 1 or 0, or 1 to [`CATEGORIES_MAX`] distinct categories.
    ///
    /// # Errors
    ///
    /// A 400 naming the rule broken.
    pub fn checked(mut self) -> Result<Checked, TelmoniError> {
        let refuse = |why: String| Err(TelmoniError::BadRequest(why));
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() || self.name.chars().count() > NAME_MAX {
            return refuse(format!("a score's name is 1 to {NAME_MAX} characters"));
        }
        match self.data_type {
            ScoreType::Number => {
                let Some(value) = self.value.filter(|v| v.is_finite()) else {
                    return refuse("a number score has a finite value".into());
                };
                let (min, max) = (self.range_min, self.range_max);
                if min.is_some_and(|m| !m.is_finite()) || max.is_some_and(|m| !m.is_finite()) {
                    return refuse("a score's range is finite".into());
                }
                if let (Some(min), Some(max)) = (min, max)
                    && min >= max
                {
                    return refuse("a score's range runs from its least to its most".into());
                }
                if min.is_some_and(|m| value < m) || max.is_some_and(|m| value > m) {
                    return refuse("a score's value is within its range".into());
                }
                if !self.categories.is_empty() {
                    return refuse("a number score names no category".into());
                }
            }
            ScoreType::Boolean => {
                if !matches!(self.value, Some(v) if v == 0.0 || v == 1.0) {
                    return refuse("a yes-or-no score is 1 or 0".into());
                }
                if !self.categories.is_empty()
                    || self.range_min.is_some()
                    || self.range_max.is_some()
                {
                    return refuse("a yes-or-no score names no category and no range".into());
                }
            }
            ScoreType::Category => {
                self.categories = self
                    .categories
                    .iter()
                    .map(|c| c.trim().to_owned())
                    .collect();
                let mut seen = std::collections::HashSet::new();
                if self.categories.is_empty()
                    || self.categories.len() > CATEGORIES_MAX
                    || self.categories.iter().any(|c| {
                        c.is_empty() || c.chars().count() > CATEGORY_MAX || !seen.insert(c)
                    })
                {
                    return refuse(format!(
                        "a category score names 1 to {CATEGORIES_MAX} distinct categories, each 1 to \
                         {CATEGORY_MAX} characters"
                    ));
                }
                if self.value.is_some() || self.range_min.is_some() || self.range_max.is_some() {
                    return refuse("a category score has no value and no range".into());
                }
            }
        }
        Ok(Checked(self))
    }

    /// The value a row keeps: a category's is 0, its categories are what it says.
    #[must_use]
    pub fn stored_value(&self) -> f64 {
        self.value.unwrap_or(0.0)
    }
}

/// A judgement held to a score's rules: only [`Judgement::checked`] makes
/// one, so no row is written from a judgement that was not.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Checked(Judgement);

impl Checked {
    /// The judgement, its fields to move into a row.
    #[must_use]
    pub fn into_inner(self) -> Judgement {
        self.0
    }
}

impl std::ops::Deref for Checked {
    type Target = Judgement;

    fn deref(&self) -> &Judgement {
        &self.0
    }
}

/// A member's score's id: one a member a name a target, so giving it again
/// replaces it.
#[must_use]
pub fn member_score_id(
    project_id: &ProjectId,
    target: &TargetKey,
    name: &str,
    member: &str,
) -> Uuid {
    Uuid::new_v5(
        &NAMESPACE,
        format!("member/{project_id}/{}/{name}/{member}", target.key()).as_bytes(),
    )
}

/// The application's score's id: one an event, by its span, its time and
/// its place among the span's scores, so an export sent again replaces its
/// scores with themselves, and two of one name at one time, as a record
/// times them, stay two.
#[must_use]
pub fn sdk_score_id(
    project_id: &ProjectId,
    trace_id: &str,
    span_id: &str,
    event: &EventScore,
) -> Uuid {
    let EventScore {
        judgement,
        at,
        place,
        ..
    } = event;
    Uuid::new_v5(
        &NAMESPACE,
        format!(
            "sdk/{project_id}/{trace_id}/{span_id}/{}/{at}/{place}",
            judgement.name
        )
        .as_bytes(),
    )
}

/// A score sent to `/v1/scores` under the caller's own id: sent again, it
/// replaces itself, and no other project's or source's id is reached.
#[must_use]
pub fn api_score_id(project_id: &ProjectId, id: Uuid) -> Uuid {
    Uuid::new_v5(&NAMESPACE, format!("api/{project_id}/{id}").as_bytes())
}

/// The application's scorer's version trimmed, a blank one none; past
/// [`VERSION_MAX`] characters, `None`.
#[must_use]
pub fn version_of(given: Option<&str>) -> Option<String> {
    let version = given.unwrap_or_default().trim();
    (version.chars().count() <= VERSION_MAX).then(|| version.to_owned())
}

/// What a score is of, by its ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetKey {
    Run(Uuid),
    Span(Uuid, String),
    Session(String),
}

impl TargetKey {
    /// What it is of.
    #[must_use]
    pub const fn kind(&self) -> ScoreTarget {
        match self {
            Self::Run(_) => ScoreTarget::Run,
            Self::Span(..) => ScoreTarget::Span,
            Self::Session(_) => ScoreTarget::Session,
        }
    }

    /// Its run, span and session, as a row keeps them: nil and empty where none.
    #[must_use]
    pub fn ids(&self) -> (Uuid, String, String) {
        match self {
            Self::Run(run) => (*run, String::new(), String::new()),
            Self::Span(run, span) => (*run, span.clone(), String::new()),
            Self::Session(session) => (Uuid::nil(), String::new(), session.clone()),
        }
    }

    fn key(&self) -> String {
        match self {
            Self::Run(run) => format!("run/{run}"),
            Self::Span(run, span) => format!("span/{run}/{span}"),
            Self::Session(session) => format!("session/{session}"),
        }
    }
}

/// A score an evaluation event gave: what it says, its scorer's version and
/// the event's time.
#[derive(Debug, Clone, PartialEq)]
pub struct EventScore {
    pub judgement: Checked,
    pub version: String,
    pub at: u64,
    /// Its place among the span's evaluation events.
    pub place: usize,
}

/// A span's scores from its evaluation events, each held to a score's rules;
/// one breaking them is left out. A number labelled is kept as its number.
#[must_use]
pub fn of_span(span: &Span) -> Vec<EventScore> {
    span.events
        .iter()
        .filter(|event| event.name == EVALUATION_EVENT)
        .enumerate()
        .filter_map(|(place, event)| {
            let attribute = |key: &str| {
                event
                    .attributes
                    .iter()
                    .find(|kv| kv.key == key)
                    .and_then(|kv| kv.value.as_ref()?.value.as_ref())
            };
            let text = |key: &str| match attribute(key) {
                Some(Value::StringValue(s)) => Some(s.clone()),
                _ => None,
            };
            let float = |key: &str| match attribute(key) {
                Some(Value::DoubleValue(v)) => Some(*v),
                #[expect(clippy::cast_precision_loss, reason = "a score's integer is small")]
                Some(Value::IntValue(v)) => Some(*v as f64),
                _ => None,
            };
            let number = match attribute(EVALUATION_VALUE) {
                Some(Value::BoolValue(v)) => Some(if *v { 1.0 } else { 0.0 }),
                _ => float(EVALUATION_VALUE),
            };
            let label = text(EVALUATION_LABEL).filter(|l| !l.trim().is_empty());
            let boolean = text(SCORE_TYPE).as_deref() == Some("boolean")
                || matches!(attribute(EVALUATION_VALUE), Some(Value::BoolValue(_)));
            let judgement = Judgement {
                name: text(EVALUATION_NAME)?,
                data_type: match (number, boolean) {
                    (Some(_), true) => ScoreType::Boolean,
                    (Some(_), false) => ScoreType::Number,
                    (None, _) => ScoreType::Category,
                },
                value: number,
                categories: if number.is_none() {
                    label.into_iter().collect()
                } else {
                    Vec::new()
                },
                range_min: float(SCORE_RANGE_MIN),
                range_max: float(SCORE_RANGE_MAX),
            };
            Some(EventScore {
                judgement: judgement.checked().ok()?,
                version: version_of(text(SCORE_VERSION).as_deref())?,
                at: event.time_unix_nano,
                place,
            })
        })
        .collect()
}

/// Another exporter's trace's scores still waiting, under the nil run.
const WAITING_SQL: &str = "SELECT * FROM telemetry.scores FINAL
     WHERE project_id IN {projects:Array(String)} AND trace_id = {trace:String}
       AND run_id = toUUID('00000000-0000-0000-0000-000000000000')";

/// Another exporter's trace landed: each waiting score of a span placed
/// written again under its run, of the run where the span is the run's own,
/// and one of a span dropped taken back. A write that fails leaves them
/// waiting, logged, for retention to take.
pub async fn place_waiting(
    store: &crate::store::Store,
    project_id: &ProjectId,
    trace_id: &str,
    placed: &[crate::store::SpanRow],
    dropped: &[String],
    token: &str,
) {
    let projects = crate::store::Projects::one(project_id.clone());
    let waiting: Vec<ScoreRow> = match store
        .read(&projects, WAITING_SQL)
        .param("trace", trace_id)
        .fetch_all()
        .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(project_id = %project_id, kind = crate::store::Fault::of(&error).as_str(),
                "scores: another exporter's waiting scores could not be read; they wait on");
            return;
        }
    };
    let now = Utc::now();
    let moved: Vec<ScoreRow> = waiting
        .into_iter()
        .filter_map(|score| {
            if let Some(row) = placed.iter().find(|row| row.span_id == score.span_id) {
                Some(ScoreRow {
                    received_at: now,
                    run_id: row.run_id,
                    session_id: row.session_id.clone(),
                    target: if row.kind == telmoni_shared::SpanKind::Run {
                        ScoreTarget::Run
                    } else {
                        ScoreTarget::Span
                    },
                    ..score
                })
            } else {
                dropped.contains(&score.span_id).then_some(ScoreRow {
                    received_at: now,
                    is_deleted: 1,
                    ..score
                })
            }
        })
        .collect();
    if let Err(error) = store
        .insert_scores_once(&moved, &format!("{token}/placed-scores"))
        .await
    {
        tracing::warn!(project_id = %project_id, kind = crate::store::Fault::of(&error).as_str(),
            "scores: another exporter's placed scores could not be written; they wait on");
    }
}

/// Some runs' own scores, a row a run a name: their mean, how many, and a
/// category's every value.
const RUN_SUMMARIES_SQL: &str = "SELECT run_id, name, any(data_type) AS data_type,
            avg(value) AS mean, count() AS n, arraySort(groupUniqArrayArray(categories)) AS categories
       FROM telemetry.scores FINAL
      WHERE project_id IN {projects:Array(String)} AND has({runs:Array(UUID)}, run_id)
        AND target = 'run'
      GROUP BY run_id, name ORDER BY name";

#[derive(Row, Deserialize)]
struct SummaryRow {
    #[serde(with = "clickhouse::serde::uuid")]
    run_id: Uuid,
    name: String,
    #[serde(with = "text")]
    data_type: ScoreType,
    mean: f64,
    n: u64,
    categories: Vec<String>,
}

/// What a list shows of one score name: its mean, but
/// a category's, how many, and a category's values.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    pub name: String,
    pub data_type: ScoreType,
    pub mean: Option<f64>,
    pub count: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
}

impl From<SummaryRow> for Summary {
    fn from(row: SummaryRow) -> Self {
        Self {
            mean: (row.data_type != ScoreType::Category).then_some(row.mean),
            name: row.name,
            data_type: row.data_type,
            count: row.n,
            categories: row.categories,
        }
    }
}

/// Each of `runs`' own scores, summed by name.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub async fn of_runs(
    store: &crate::store::Store,
    projects: &crate::store::Projects,
    runs: &[Uuid],
) -> clickhouse::error::Result<std::collections::BTreeMap<Uuid, Vec<Summary>>> {
    let mut by_run = std::collections::BTreeMap::<Uuid, Vec<Summary>>::new();
    if runs.is_empty() {
        return Ok(by_run);
    }
    let rows: Vec<SummaryRow> = store
        .read(projects, RUN_SUMMARIES_SQL)
        .param("runs", runs)
        .fetch_all()
        .await?;
    for row in rows {
        by_run.entry(row.run_id).or_default().push(row.into());
    }
    Ok(by_run)
}

/// A run's scores, and its spans' and its session's, newest first, each
/// once as given last; those taken back left out.
pub const RUN_SQL: &str = "SELECT * FROM telemetry.scores FINAL
     WHERE project_id IN {projects:Array(String)} AND is_deleted = 0
       AND (run_id = {run:UUID} OR (target = 'session' AND session_id != ''
            AND session_id = {session:String}))
     ORDER BY received_at DESC
     LIMIT 1000";

#[cfg(test)]
mod tests {
    use super::*;

    fn judgement(data_type: ScoreType) -> Judgement {
        Judgement {
            name: "helpful".into(),
            data_type,
            value: None,
            categories: Vec::new(),
            range_min: None,
            range_max: None,
        }
    }

    /// A score is held to its type's rules, a name trimmed and a category's too.
    #[test]
    fn a_score_is_held_to_its_types_rules() {
        let number = |value: f64, min: Option<f64>, max: Option<f64>| Judgement {
            value: Some(value),
            range_min: min,
            range_max: max,
            ..judgement(ScoreType::Number)
        };
        assert!(number(0.5, Some(0.0), Some(1.0)).checked().is_ok());
        assert!(number(2.0, Some(0.0), Some(1.0)).checked().is_err());
        assert!(number(f64::NAN, None, None).checked().is_err());
        assert!(number(1.0, Some(1.0), Some(1.0)).checked().is_err());
        let boolean = |v: f64| Judgement {
            value: Some(v),
            ..judgement(ScoreType::Boolean)
        };
        assert!(boolean(1.0).checked().is_ok());
        assert!(boolean(0.5).checked().is_err());
        let category = |c: &[&str]| Judgement {
            categories: c.iter().map(|s| (*s).to_owned()).collect(),
            ..judgement(ScoreType::Category)
        };
        assert_eq!(
            category(&[" on topic "]).checked().expect("one").categories,
            ["on topic"]
        );
        assert!(category(&[]).checked().is_err());
        assert!(category(&["a", "a"]).checked().is_err());
        assert!(category(&["a"; CATEGORIES_MAX + 1]).checked().is_err());
        let named = Judgement {
            name: "  ".into(),
            ..boolean(1.0)
        };
        assert!(named.checked().is_err());
    }

    /// A member's score is one a name a target a member; the application's
    /// one an event.
    #[test]
    fn a_scores_id_is_one_a_scorer_a_name_a_target() {
        let project = ProjectId::new();
        let run = TargetKey::Run(Uuid::nil());
        assert_eq!(
            member_score_id(&project, &run, "helpful", "u1"),
            member_score_id(&project, &run, "helpful", "u1")
        );
        assert_ne!(
            member_score_id(&project, &run, "helpful", "u1"),
            member_score_id(&project, &run, "helpful", "u2")
        );
        let event = |at, place| EventScore {
            judgement: Checked(judgement(ScoreType::Boolean)),
            version: String::new(),
            at,
            place,
        };
        assert_ne!(
            sdk_score_id(&project, "t", "s", &event(1, 0)),
            sdk_score_id(&project, "t", "s", &event(2, 0))
        );
        assert_ne!(
            sdk_score_id(&project, "t", "s", &event(1, 0)),
            sdk_score_id(&project, "t", "s", &event(1, 1)),
            "two of one name at one time stay two"
        );
    }
}
