//! Scores: a judgement of a run, one of its spans or a session, as a name and
//! a number, a yes or no, or a category. On a span still open it rides as
//! OpenTelemetry's `gen_ai.evaluation.result` event; once ended, the client
//! sends it to `/v1/scores`, through the export queue as any span goes, so a
//! flush and a shutdown hold it alike (`sdk/fixtures/scores.json`).

use opentelemetry::KeyValue;
use uuid::Uuid;

use crate::contract;
use crate::warnings::{Warner, WarningKind};

/// A score, built by [`Score::number`], [`Score::boolean`] or
/// [`Score::category`], then given to a handle's `score` or the client's.
#[derive(Debug, Clone, PartialEq)]
pub struct Score {
    name: String,
    value: Value,
    range: Option<(f64, f64)>,
    version: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Number(f64),
    Boolean(bool),
    Category(String),
}

impl Score {
    /// A number, finite, within its [`range`](Self::range) where it has one.
    #[must_use]
    pub fn number(name: impl Into<String>, value: f64) -> Self {
        Self::of(name, Value::Number(value))
    }

    /// A yes or no.
    #[must_use]
    pub fn boolean(name: impl Into<String>, value: bool) -> Self {
        Self::of(name, Value::Boolean(value))
    }

    /// One category of a set, its label 1 to 64 characters.
    #[must_use]
    pub fn category(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::of(name, Value::Category(label.into()))
    }

    /// The range a number score falls within, its ends included.
    #[must_use]
    pub const fn range(mut self, min: f64, max: f64) -> Self {
        self.range = Some((min, max));
        self
    }

    /// The version of what scored, up to 64 characters.
    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    fn of(name: impl Into<String>, value: Value) -> Self {
        Self {
            name: name.into(),
            value,
            range: None,
            version: None,
        }
    }

    /// The score as it is sent, or why it is not.
    fn checked(&self) -> Result<Checked, &'static str> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > contract::SCORE_NAME_MAX {
            return Err("its name is 1 to 64 characters");
        }
        let version = self.version.as_deref().map(str::trim).unwrap_or_default();
        if version.chars().count() > contract::SCORE_VERSION_MAX {
            return Err("its version is up to 64 characters");
        }
        let value = match &self.value {
            Value::Number(value) => {
                let within = self.range.is_none_or(|(min, max)| {
                    min.is_finite()
                        && max.is_finite()
                        && min < max
                        && min <= *value
                        && *value <= max
                });
                if !value.is_finite() || !within {
                    return Err("a number score is a finite number within its range");
                }
                Value::Number(*value)
            }
            _ if self.range.is_some() => return Err("a range is a number score's alone"),
            Value::Boolean(value) => Value::Boolean(*value),
            Value::Category(label) => {
                let label = label.trim();
                if label.is_empty() || label.chars().count() > contract::SCORE_CATEGORY_MAX {
                    return Err("its category is 1 to 64 characters");
                }
                Value::Category(label.to_owned())
            }
        };
        Ok(Checked {
            name: name.to_owned(),
            value,
            range: self.range,
            version: (!version.is_empty()).then(|| version.to_owned()),
        })
    }
}

/// What a score sent once its span ended is of: a run, one of its spans, or
/// a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreTarget {
    run_id: Option<String>,
    span_id: Option<String>,
    session_id: Option<String>,
}

impl ScoreTarget {
    /// A run, by its id, a UUID.
    #[must_use]
    pub fn run(run_id: impl Into<String>) -> Self {
        Self {
            run_id: Some(run_id.into()),
            span_id: None,
            session_id: None,
        }
    }

    /// One span of a run, by the run's id and the span's 16 hex digits.
    #[must_use]
    pub fn span(run_id: impl Into<String>, span_id: impl Into<String>) -> Self {
        Self {
            span_id: Some(span_id.into()),
            ..Self::run(run_id)
        }
    }

    /// A session, by its id.
    #[must_use]
    pub fn session(session_id: impl Into<String>) -> Self {
        Self {
            run_id: None,
            span_id: None,
            session_id: Some(session_id.into()),
        }
    }

    /// Its ids as `/v1/scores` takes them, in lowercase, or `None`.
    fn checked(&self) -> Option<serde_json::Map<String, serde_json::Value>> {
        let mut ids = serde_json::Map::new();
        let session = self.session_id.as_deref().map(str::trim);
        match (&self.run_id, &self.span_id, session) {
            (Some(run), span, None) => {
                let run = Uuid::parse_str(run).ok().filter(|run| !run.is_nil())?;
                ids.insert("run_id".into(), run.to_string().into());
                if let Some(span) = span {
                    if span.len() != 16 || !span.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return None;
                    }
                    ids.insert("span_id".into(), span.to_ascii_lowercase().into());
                }
            }
            (None, None, Some(session))
                if !session.is_empty() && session.chars().count() <= contract::SESSION_ID_MAX =>
            {
                ids.insert("session_id".into(), session.into());
            }
            _ => return None,
        }
        Some(ids)
    }
}

/// A score held to its rules: its name, label and version trimmed.
struct Checked {
    name: String,
    value: Value,
    range: Option<(f64, f64)>,
    version: Option<String>,
}

impl Checked {
    /// The evaluation event's attributes.
    fn attributes(self) -> Vec<KeyValue> {
        let mut attributes = vec![KeyValue::new(contract::EVALUATION_NAME, self.name)];
        match self.value {
            Value::Number(value) => {
                attributes.push(KeyValue::new(contract::EVALUATION_VALUE, value));
            }
            Value::Boolean(value) => {
                attributes.push(KeyValue::new(
                    contract::EVALUATION_VALUE,
                    if value { 1.0 } else { 0.0 },
                ));
                attributes.push(KeyValue::new(contract::SCORE_TYPE, "boolean"));
            }
            Value::Category(label) => {
                attributes.push(KeyValue::new(contract::EVALUATION_LABEL, label));
            }
        }
        if let Some((min, max)) = self.range {
            attributes.push(KeyValue::new(contract::SCORE_RANGE_MIN, min));
            attributes.push(KeyValue::new(contract::SCORE_RANGE_MAX, max));
        }
        if let Some(version) = self.version {
            attributes.push(KeyValue::new(contract::SCORE_VERSION, version));
        }
        attributes
    }

    /// One item of `/v1/scores`'s body, under a fresh id, so a retry replaces itself.
    fn sent(self, mut item: serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
        item.insert("id".into(), Uuid::new_v4().to_string().into());
        item.insert("name".into(), self.name.into());
        let (data_type, value) = match self.value {
            Value::Number(value) => ("number", Some(value.into())),
            Value::Boolean(value) => ("boolean", Some(u8::from(value).into())),
            Value::Category(label) => {
                item.insert("categories".into(), vec![label].into());
                ("category", None)
            }
        };
        item.insert("data_type".into(), data_type.into());
        if let Some(value) = value {
            item.insert("value".into(), value);
        }
        if let Some((min, max)) = self.range {
            item.insert("range_min".into(), min.into());
            item.insert("range_max".into(), max.into());
        }
        if let Some(version) = self.version {
            item.insert("version".into(), version.into());
        }
        serde_json::Value::Object(item)
    }
}

/// Warns once of a score not sent, and why.
fn refused(warner: &Warner, why: &str) {
    warner.once(
        format!("score:{why}"),
        &format!("telmoni: a score was not sent: {why}"),
        WarningKind::Input.into(),
    );
}

/// The event's attributes for `score` on a span holding `held` scores
/// already, or `None`, warned of.
pub(crate) fn event(score: &Score, held: usize, warner: &Warner) -> Option<Vec<KeyValue>> {
    let checked = score.checked().map_err(|why| refused(warner, why)).ok()?;
    if held >= contract::SCORES_PER_SPAN_MAX {
        refused(warner, "its span holds 16 scores already");
        return None;
    }
    Some(checked.attributes())
}

/// The JSON `/v1/scores` takes for `score` of `target`, or `None`, warned of.
pub(crate) fn sent(target: &ScoreTarget, score: &Score, warner: &Warner) -> Option<String> {
    let Some(ids) = target.checked() else {
        refused(
            warner,
            "it names a run, a run and one of its spans by 16 hex digits, or a session of up to 200 characters",
        );
        return None;
    };
    let checked = score.checked().map_err(|why| refused(warner, why)).ok()?;
    Some(checked.sent(ids).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixtures_limits_are_the_sdks() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/scores.json");
        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("scores.json"))
                .expect("scores.json is JSON");
        let limits = &fixture["limits"];
        assert_eq!(limits["name_max"], contract::SCORE_NAME_MAX);
        assert_eq!(limits["category_max"], contract::SCORE_CATEGORY_MAX);
        assert_eq!(limits["version_max"], contract::SCORE_VERSION_MAX);
        assert_eq!(limits["session_max"], contract::SESSION_ID_MAX);
        assert_eq!(limits["per_span_max"], contract::SCORES_PER_SPAN_MAX);
        assert_eq!(limits["sent_max"], contract::SCORES_SENT_MAX);
    }
}
