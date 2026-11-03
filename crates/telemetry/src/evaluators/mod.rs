//! Evaluators, LLM-as-a-judge: a prompt whose `{{variables}}` are filled from
//! what is judged, the score it gives, each end or category described for the
//! model, and the reasoning it asks for. The provider is asked for JSON in the
//! score's own schema; an answer outside it judges nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use telmoni_shared::model::Role;
use telmoni_shared::{ScoreType, SpanKind, TelmoniError};

pub mod agreement;
pub mod examples;
pub mod run;
pub mod typed;

use crate::ingest::batch::SpanContent;
use crate::scores::{CATEGORIES_MAX, CATEGORY_MAX, Checked, Judgement, NAME_MAX as SCORE_NAME_MAX};
use crate::store::SpanRow;

/// The longest name an evaluator goes by, the most messages its prompt holds,
/// the longest each runs to, and the longest description of a score's end.
pub const NAME_MAX: usize = 60;
pub const MESSAGES_MAX: usize = 20;
pub const MESSAGE_MAX: usize = 20_000;
pub const MEANS_MAX: usize = 500;

/// The variables a prompt may name: a span's or a run's, a run's
/// trajectory, a session's conversation, and its corrections as examples.
pub const VARIABLES: &[&str] = &[
    "input",
    "output",
    "metadata",
    "tool_calls",
    "trajectory",
    "messages",
    "exchanges",
    "first_and_last",
    "examples",
];

/// The variables a run fills.
pub const RUN_VARIABLES: &[&str] = &["input", "output", "metadata", "tool_calls", "trajectory"];

/// The most a variable is given of the content it reads, so one long run
/// leaves the model room to answer.
pub const FILLED_MAX: usize = 60_000;

/// Who says a prompt's message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptRole {
    System,
    User,
    Assistant,
}

telmoni_shared::wire_enum!(PromptRole {
    System => "system",
    User => "user",
    Assistant => "assistant",
});

/// One message of a prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: PromptRole,
    pub content: String,
}

/// One category a score may give, and what it means.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    pub name: String,
    pub means: String,
}

/// The score an evaluator gives, as 8.1 keeps it, each of its ends or
/// categories described for the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoreSpec {
    pub name: String,
    pub data_type: ScoreType,
    #[serde(default)]
    pub range_min: Option<f64>,
    #[serde(default)]
    pub range_max: Option<f64>,
    /// What a number's least means, or a yes-or-no's no.
    #[serde(default)]
    pub low_means: String,
    /// What a number's most means, or a yes-or-no's yes.
    #[serde(default)]
    pub high_means: String,
    #[serde(default)]
    pub categories: Vec<Category>,
    /// Several categories at once, rather than one.
    #[serde(default)]
    pub multiple: bool,
}

/// What an evaluator asks: its messages, its score, and whether it asks for
/// the reasoning that becomes the score's comment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    pub messages: Vec<Message>,
    pub score: ScoreSpec,
    #[serde(default = "yes")]
    pub reasoning: bool,
}

const fn yes() -> bool {
    true
}

/// Whether `inner`, the text between a pair of braces, names a variable.
fn is_name(inner: &str) -> bool {
    !inner.is_empty()
        && inner
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
}

/// Each `{{ name }}` a text names, in order, as written between the braces.
fn named(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some((_, after)) = rest.split_once("{{") {
        let Some((inner, tail)) = after.split_once("}}") else {
            break;
        };
        if is_name(inner.trim()) {
            found.push(inner.trim());
        }
        rest = tail;
    }
    found
}

/// `text` with each `{{ name }}` `filled` holds replaced, the rest as written.
fn fill(text: &str, filled: &BTreeMap<&str, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("{{") {
        out.push_str(before);
        let Some((inner, tail)) = after.split_once("}}") else {
            out.push_str("{{");
            out.push_str(after);
            return out;
        };
        match filled.get(inner.trim()) {
            Some(value) => out.push_str(value),
            None => {
                out.push_str("{{");
                out.push_str(inner);
                out.push_str("}}");
            }
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// A number written as a person would: `1`, not `1.0`.
fn number(n: f64) -> String {
    let shown = format!("{n}");
    shown
        .strip_suffix(".0")
        .map_or(shown.clone(), str::to_owned)
}

impl ScoreSpec {
    /// The score held to its type's rules, each text trimmed: a number's two
    /// ends and what each means, a yes-or-no's two, or 2 to
    /// [`CATEGORIES_MAX`] distinct categories, each described.
    fn checked(mut self) -> Result<Self, String> {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() || self.name.chars().count() > SCORE_NAME_MAX {
            return Err(format!(
                "a score's name is 1 to {SCORE_NAME_MAX} characters"
            ));
        }
        self.low_means = self.low_means.trim().to_owned();
        self.high_means = self.high_means.trim().to_owned();
        let described = |means: &str| !means.is_empty() && means.chars().count() <= MEANS_MAX;
        match self.data_type {
            ScoreType::Number => {
                let (Some(min), Some(max)) = (self.range_min, self.range_max) else {
                    return Err("an evaluator's number score names its least and its most".into());
                };
                if !min.is_finite() || !max.is_finite() || min >= max {
                    return Err("a score's range runs from its least to its most".into());
                }
            }
            ScoreType::Boolean | ScoreType::Category => {
                if self.range_min.is_some() || self.range_max.is_some() {
                    return Err("only a number score names a range".into());
                }
            }
        }
        if self.data_type == ScoreType::Category {
            if !self.low_means.is_empty() || !self.high_means.is_empty() {
                return Err("a category score describes each category instead".into());
            }
            for category in &mut self.categories {
                category.name = category.name.trim().to_owned();
                category.means = category.means.trim().to_owned();
                if category.name.is_empty()
                    || category.name.chars().count() > CATEGORY_MAX
                    || !described(&category.means)
                {
                    return Err(format!(
                        "each category is named in 1 to {CATEGORY_MAX} characters and described in up to {MEANS_MAX}"
                    ));
                }
            }
            let distinct: BTreeSet<&str> =
                self.categories.iter().map(|c| c.name.as_str()).collect();
            if !(2..=CATEGORIES_MAX).contains(&self.categories.len())
                || distinct.len() != self.categories.len()
            {
                return Err(format!(
                    "a category score names 2 to {CATEGORIES_MAX} distinct categories"
                ));
            }
        } else {
            if !self.categories.is_empty() || self.multiple {
                return Err("only a category score names categories".into());
            }
            if !described(&self.low_means) || !described(&self.high_means) {
                return Err(format!(
                    "a score describes what each of its ends means, in up to {MEANS_MAX} characters"
                ));
            }
        }
        Ok(self)
    }

    /// The JSON Schema of the score alone.
    fn schema(&self) -> Value {
        let names = || {
            self.categories
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        };
        match self.data_type {
            ScoreType::Number => json!({
                "type": "number",
                "minimum": self.range_min,
                "maximum": self.range_max,
            }),
            ScoreType::Boolean => json!({ "type": "boolean" }),
            ScoreType::Category if self.multiple => json!({
                "type": "array",
                "items": { "type": "string", "enum": names() },
            }),
            ScoreType::Category => json!({ "type": "string", "enum": names() }),
        }
    }

    /// The score described for the model, as the instructions end.
    fn described(&self) -> String {
        let mut said = format!("The score is \"{}\". ", self.name);
        match self.data_type {
            ScoreType::Number => {
                let (min, max) = (
                    number(self.range_min.unwrap_or_default()),
                    number(self.range_max.unwrap_or_default()),
                );
                said.push_str(&format!(
                    "It is a number from {min} to {max}: {min} means {}; {max} means {}.",
                    self.low_means, self.high_means
                ));
            }
            ScoreType::Boolean => said.push_str(&format!(
                "It is true or false: true means {}; false means {}.",
                self.high_means, self.low_means
            )),
            ScoreType::Category => {
                said.push_str(if self.multiple {
                    "It is every category that applies, of these:"
                } else {
                    "It is the one category that applies, of these:"
                });
                for category in &self.categories {
                    said.push_str(&format!("\n- {}: {}", category.name, category.means));
                }
            }
        }
        said
    }
}

impl Prompt {
    /// The prompt held to an evaluator's rules: 1 to [`MESSAGES_MAX`]
    /// messages, a system message only first, a user's among them, each
    /// variable one of [`VARIABLES`], and the score held to its own.
    ///
    /// # Errors
    ///
    /// A 400 naming the rule broken.
    pub fn checked(mut self) -> Result<Self, TelmoniError> {
        let refuse = |why: String| Err(TelmoniError::BadRequest(why));
        if self.messages.is_empty() || self.messages.len() > MESSAGES_MAX {
            return refuse(format!("a prompt holds 1 to {MESSAGES_MAX} messages"));
        }
        for (at, message) in self.messages.iter_mut().enumerate() {
            message.content = message.content.trim().to_owned();
            if message.content.is_empty() || message.content.chars().count() > MESSAGE_MAX {
                return refuse(format!(
                    "each message runs to 1 to {MESSAGE_MAX} characters"
                ));
            }
            if message.role == PromptRole::System && at > 0 {
                return refuse("a prompt's system message comes first, once".into());
            }
        }
        if !self.messages.iter().any(|m| m.role == PromptRole::User) {
            return refuse("a prompt holds the user's message the judged is shown in".into());
        }
        if let Some(unknown) = self
            .variables()
            .into_iter()
            .find(|v| !VARIABLES.contains(&v.as_str()))
        {
            return refuse(format!(
                "{{{{{unknown}}}}} is no variable; a prompt names {}",
                VARIABLES
                    .iter()
                    .map(|v| format!("{{{{{v}}}}}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        self.score = self.score.checked().map_err(TelmoniError::BadRequest)?;
        Ok(self)
    }

    /// Each variable the prompt names, once.
    pub fn variables(&self) -> BTreeSet<String> {
        self.messages
            .iter()
            .flat_map(|m| named(&m.content))
            .map(str::to_owned)
            .collect()
    }

    /// The JSON Schema the provider is asked to answer in: the reasoning
    /// first, where asked for, so the model reasons before it scores.
    pub fn schema(&self) -> Value {
        if self.reasoning {
            json!({
                "type": "object",
                "properties": {
                    "reasoning": { "type": "string" },
                    "score": self.score.schema(),
                },
                "required": ["reasoning", "score"],
                "additionalProperties": false,
            })
        } else {
            json!({
                "type": "object",
                "properties": { "score": self.score.schema() },
                "required": ["score"],
                "additionalProperties": false,
            })
        }
    }

    /// The prompt as sent: its system message, the score described after it,
    /// and the rest, each variable `filled` holds replaced.
    pub fn rendered(&self, filled: &BTreeMap<&str, String>) -> (String, Vec<(Role, String)>) {
        let mut system = String::new();
        let mut messages = Vec::new();
        for message in &self.messages {
            let text = fill(&message.content, filled);
            match message.role {
                PromptRole::System => system = text,
                PromptRole::User => messages.push((Role::User, text)),
                PromptRole::Assistant => messages.push((Role::Assistant, text)),
            }
        }
        if !system.is_empty() {
            system.push_str("\n\n");
        }
        system.push_str(&self.score.described());
        system.push_str(if self.reasoning {
            "\n\nGive your reasoning first, in a few sentences, then the score. Answer only with the JSON asked for."
        } else {
            "\n\nAnswer only with the JSON asked for."
        });
        (system, messages)
    }

    /// The model's answer read as the score it gives and its reasoning.
    ///
    /// # Errors
    ///
    /// An answer outside the score's schema, named.
    pub fn judgement(&self, answer: &Value) -> Result<(Checked, Option<String>), String> {
        let outside = |why: &str| format!("the model answered outside the score's schema: {why}");
        let reasoning = answer
            .get("reasoning")
            .and_then(Value::as_str)
            .map(|r| r.trim().to_owned())
            .filter(|r| !r.is_empty());
        if self.reasoning && reasoning.is_none() {
            return Err(outside("no reasoning"));
        }
        let score = answer.get("score").ok_or_else(|| outside("no score"))?;
        let spec = &self.score;
        let (value, categories) = match spec.data_type {
            ScoreType::Number => (
                Some(
                    score
                        .as_f64()
                        .ok_or_else(|| outside("the score is no number"))?,
                ),
                Vec::new(),
            ),
            ScoreType::Boolean => (
                Some(
                    if score
                        .as_bool()
                        .ok_or_else(|| outside("the score is no true or false"))?
                    {
                        1.0
                    } else {
                        0.0
                    },
                ),
                Vec::new(),
            ),
            ScoreType::Category => {
                let given: Vec<String> = match score {
                    Value::String(one) if !spec.multiple => vec![one.clone()],
                    Value::Array(many) if spec.multiple => many
                        .iter()
                        .map(|c| c.as_str().map(str::to_owned))
                        .collect::<Option<_>>()
                        .ok_or_else(|| outside("a category is no text"))?,
                    _ => return Err(outside("the score is no category")),
                };
                if given
                    .iter()
                    .any(|g| !spec.categories.iter().any(|c| &c.name == g))
                {
                    return Err(outside("a category it does not name"));
                }
                let mut given = given;
                given.dedup();
                (None, given)
            }
        };
        let judgement = Judgement {
            name: spec.name.clone(),
            data_type: spec.data_type,
            value,
            categories,
            range_min: spec.range_min,
            range_max: spec.range_max,
        }
        .checked()
        .map_err(|e| outside(&e.to_string()))?;
        Ok((judgement, reasoning))
    }
}

/// What an evaluator asks, held to its kind's rules.
#[derive(Debug, Clone)]
pub enum Judging {
    Prompt(Prompt),
    Typed(typed::Typed),
}

/// Why an evaluator of `judging`'s kind cannot judge on a connection to
/// `provider`, if it cannot: a typed one speaks System One alone, and a
/// prompt every other protocol.
#[must_use]
pub fn misfit(provider: telmoni_shared::ModelProvider, judging: &Judging) -> Option<&'static str> {
    match (telmoni_shared::model::speaks_system_one(provider), judging) {
        (false, Judging::Typed(_)) => Some(
            "a typed evaluator judges on a System One connection, TypeSafe's or an endpoint's speaking its shape",
        ),
        (true, Judging::Prompt(_)) => Some(
            "a prompt evaluator judges on a connection that writes, not on a System One one, which answers typed questions alone",
        ),
        _ => None,
    }
}

impl Judging {
    /// Each variable it names, once.
    pub fn variables(&self) -> BTreeSet<String> {
        match self {
            Self::Prompt(prompt) => prompt.variables(),
            Self::Typed(typed) => typed.variables(),
        }
    }
}

/// An evaluator to copy, never linked: a prompt and a score, in a group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    pub key: String,
    pub group: String,
    pub name: String,
    pub description: String,
    #[serde(flatten)]
    pub prompt: Prompt,
}

/// The templates, as `templates/` holds them.
const TEMPLATE_FILES: &[&str] = &[
    include_str!("templates/hallucination.json"),
    include_str!("templates/correctness.json"),
    include_str!("templates/answer_relevance.json"),
    include_str!("templates/conciseness.json"),
    include_str!("templates/helpfulness.json"),
    include_str!("templates/toxicity.json"),
    include_str!("templates/bias.json"),
    include_str!("templates/pii_leakage.json"),
    include_str!("templates/prompt_injection.json"),
    include_str!("templates/task_completion.json"),
    include_str!("templates/user_satisfaction.json"),
    include_str!("templates/tone.json"),
    include_str!("templates/perceived_error.json"),
    include_str!("templates/tool_selection.json"),
    include_str!("templates/plan_adherence.json"),
    include_str!("templates/trajectory_accuracy.json"),
];

/// Every template, in the library's order; one that does not read is left out
/// and logged, which the suite's test makes a failed build instead.
pub static TEMPLATES: LazyLock<Vec<Template>> = LazyLock::new(|| {
    TEMPLATE_FILES
        .iter()
        .filter_map(|file| match serde_json::from_str::<Template>(file) {
            Ok(template) => Some(template),
            Err(error) => {
                tracing::error!(%error, "telemetry: an evaluator template does not read");
                None
            }
        })
        .collect()
});

/// A typed evaluator to copy, never linked: a state and its questions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypedTemplate {
    pub key: String,
    pub group: String,
    pub name: String,
    pub description: String,
    #[serde(flatten)]
    pub typed: typed::Typed,
}

/// The typed templates, as `templates/typed/` holds them.
const TYPED_TEMPLATE_FILES: &[&str] = &[
    include_str!("templates/typed/tool_selection.json"),
    include_str!("templates/typed/task_completion.json"),
    include_str!("templates/typed/scope_adherence.json"),
    include_str!("templates/typed/groundedness.json"),
    include_str!("templates/typed/prompt_injection.json"),
    include_str!("templates/typed/pii_in_answer.json"),
];

/// Every typed template, read as [`TEMPLATES`] are.
pub static TYPED_TEMPLATES: LazyLock<Vec<TypedTemplate>> = LazyLock::new(|| {
    TYPED_TEMPLATE_FILES
        .iter()
        .filter_map(|file| match serde_json::from_str::<TypedTemplate>(file) {
            Ok(template) => Some(template),
            Err(error) => {
                tracing::error!(%error, "telemetry: a typed evaluator template does not read");
                None
            }
        })
        .collect()
});

/// The most files a judgement sends, and the most bytes they hold between
/// them, beside each provider's own limit for each kind (`model::takes`).
pub const FILES_MAX: usize = 5;
pub const FILES_BYTES_MAX: usize = 10 * 1024 * 1024;

/// The files `texts` name as kept here, each weighed by its kind and size
/// first, then opened — as `actor`'s read, its chain row filed, or as an
/// evaluation's with none — where `provider` takes it, up to [`FILES_MAX`]
/// and [`FILES_BYTES_MAX`]; and how many it named that are not sent.
///
/// # Errors
///
/// As [`content::open_media`].
pub(crate) async fn files_of<'t>(
    state: &crate::AppState,
    actor: Option<telmoni_shared::audit::Actor<'_>>,
    project_id: &telmoni_shared::ProjectId,
    texts: impl Iterator<Item = &'t str>,
    provider: telmoni_shared::ModelProvider,
) -> Result<(Vec<telmoni_shared::model::Media>, usize), TelmoniError> {
    let mut hashes: Vec<String> = Vec::new();
    for text in texts {
        for hash in crate::content::files_named(text) {
            if !hashes.contains(&hash) {
                hashes.push(hash);
            }
        }
    }
    let mut sent: Vec<telmoni_shared::model::Media> = Vec::new();
    let mut bytes = 0;
    let mut left = hashes.len().saturating_sub(FILES_MAX);
    for hash in hashes.iter().take(FILES_MAX) {
        let fits = crate::content::media_kind(state, project_id, hash)
            .await?
            .is_some_and(|(mime_type, size)| {
                telmoni_shared::model::takes(provider, &mime_type, size)
                    && bytes + size <= FILES_BYTES_MAX
            });
        let opened = if !fits {
            None
        } else if let Some(actor) = actor {
            crate::content::open_media(state, actor, project_id, hash).await?
        } else {
            crate::content::open_media_for_evaluation(state, project_id, hash).await?
        };
        match opened {
            Some((mime_type, body)) => {
                bytes += body.len();
                sent.push(telmoni_shared::model::Media {
                    mime_type,
                    bytes: body,
                });
            }
            None => left += 1,
        }
    }
    Ok((sent, left))
}

/// The last user's message told how many files it names were not sent, so
/// the judge knows it saw part of what was judged.
pub fn noted(messages: &mut [(Role, String)], left: usize) {
    if left == 0 {
        return;
    }
    if let Some((_, text)) = messages
        .iter_mut()
        .rev()
        .find(|(role, _)| *role == Role::User)
    {
        text.push_str(&format!(
            "\n\n[{left} of the files named here were not sent: past {FILES_MAX} files or {} MiB, gone, or of a kind this model's provider takes none of]",
            FILES_BYTES_MAX / (1024 * 1024)
        ));
    }
}

/// `text` cut to [`FILLED_MAX`] characters, saying so.
fn bounded(text: String) -> String {
    if text.chars().count() <= FILLED_MAX {
        return text;
    }
    let mut cut: String = text.chars().take(FILLED_MAX).collect();
    cut.push_str("\n[cut: the rest was not sent]");
    cut
}

/// The variables a run fills, from its spans in the order they started and
/// its opened content: its input and output, its own span's where kept, else
/// its first model call's input and its last one's output; its metadata; its
/// tool calls with their arguments; and its trajectory, each tool call with
/// its arguments and result in order.
pub fn of_run(spans: &[SpanRow], content: &[SpanContent]) -> BTreeMap<&'static str, String> {
    let field = |span_id: &str, name: &str| {
        content
            .iter()
            .find(|c| c.span_id == span_id)
            .and_then(|c| c.fields.iter().find(|(f, _)| f == name))
            .map(|(_, text)| text.clone())
    };
    let run = spans.iter().find(|s| s.kind == SpanKind::Run);
    let calls: Vec<&SpanRow> = spans
        .iter()
        .filter(|s| s.kind == SpanKind::ModelCall)
        .collect();
    let input = run
        .and_then(|r| field(&r.span_id, "gen_ai.input.messages"))
        .or_else(|| {
            calls
                .iter()
                .find_map(|c| field(&c.span_id, "gen_ai.input.messages"))
        })
        .unwrap_or_default();
    let output = run
        .and_then(|r| field(&r.span_id, "gen_ai.output.messages"))
        .or_else(|| {
            calls
                .iter()
                .rev()
                .find_map(|c| field(&c.span_id, "gen_ai.output.messages"))
        })
        .unwrap_or_default();
    let tools: Vec<&SpanRow> = spans
        .iter()
        .filter(|s| s.kind == SpanKind::ToolCall)
        .collect();
    let tool_calls: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "arguments": field(&t.span_id, "gen_ai.tool.call.arguments"),
            })
        })
        .collect();
    let trajectory: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "arguments": field(&t.span_id, "gen_ai.tool.call.arguments"),
                "result": field(&t.span_id, "gen_ai.tool.call.result"),
                "status": t.status.to_string(),
            })
        })
        .collect();
    let metadata = run.map_or(Value::Null, |r| {
        json!({
            "agent": r.agent,
            "status": r.status.to_string(),
            "error_type": r.error_type,
            "seconds": (r.ended_at - r.started_at).as_seconds_f64(),
            "models": calls.iter().map(|c| c.model.as_str()).filter(|m| !m.is_empty()).collect::<BTreeSet<_>>(),
            "input_tokens": spans.iter().map(|s| s.input_tokens).sum::<u64>(),
            "output_tokens": spans.iter().map(|s| s.output_tokens).sum::<u64>(),
            "environment": r.environment,
            "release": r.release,
            "tags": r.tags.iter().cloned().collect::<BTreeMap<_, _>>(),
        })
    });
    let written = |value: Vec<Value>| serde_json::to_string_pretty(&value).unwrap_or_default();
    BTreeMap::from([
        ("input", bounded(input)),
        ("output", bounded(output)),
        (
            "metadata",
            serde_json::to_string_pretty(&metadata).unwrap_or_default(),
        ),
        ("tool_calls", bounded(written(tool_calls))),
        ("trajectory", bounded(written(trajectory))),
    ])
}

/// One field of one span's opened content.
fn field_of(content: &[SpanContent], span_id: &str, name: &str) -> Option<String> {
    content
        .iter()
        .find(|c| c.span_id == span_id)
        .and_then(|c| c.fields.iter().find(|(f, _)| f == name))
        .map(|(_, text)| text.clone())
}

/// The variables a span fills: its input and output — a tool call's
/// arguments and result — its metadata, and the tool calls it made or is.
pub fn of_span(span: &SpanRow, content: &[SpanContent]) -> BTreeMap<&'static str, String> {
    let field = |name: &str| field_of(content, &span.span_id, name);
    let tool = span.kind == SpanKind::ToolCall;
    let input = if tool {
        field("gen_ai.tool.call.arguments")
    } else {
        field("gen_ai.input.messages")
    }
    .unwrap_or_default();
    let output = if tool {
        field("gen_ai.tool.call.result")
    } else {
        field("gen_ai.output.messages")
    }
    .unwrap_or_default();
    let tool_calls: Vec<Value> = if tool {
        vec![json!({ "name": span.name, "arguments": field("gen_ai.tool.call.arguments") })]
    } else {
        // The calls a model's answer asked for, as its output's parts name them.
        serde_json::from_str::<Value>(&output)
            .ok()
            .and_then(|messages| messages.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .flat_map(|m| {
                m.get("parts")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool_call"))
            .collect()
    };
    let metadata = json!({
        "name": span.name,
        "kind": span.kind.to_string(),
        "status": span.status.to_string(),
        "error_type": span.error_type,
        "model": span.model,
        "seconds": (span.ended_at - span.started_at).as_seconds_f64(),
        "input_tokens": span.input_tokens,
        "output_tokens": span.output_tokens,
        "attributes": span.attributes.iter().cloned().collect::<BTreeMap<_, _>>(),
    });
    BTreeMap::from([
        ("input", bounded(input)),
        ("output", bounded(output)),
        (
            "metadata",
            serde_json::to_string_pretty(&metadata).unwrap_or_default(),
        ),
        (
            "tool_calls",
            bounded(serde_json::to_string_pretty(&tool_calls).unwrap_or_default()),
        ),
    ])
}

/// The variables a session fills, from each of its runs in order, its spans
/// and opened content: every message, the person's and the agent's in
/// pairs, and the first prompt with the last answer.
pub fn of_session(runs: &[(Vec<SpanRow>, Vec<SpanContent>)]) -> BTreeMap<&'static str, String> {
    let turns: Vec<(String, String)> = runs
        .iter()
        .map(|(spans, content)| {
            let filled = of_run(spans, content);
            (
                filled.get("input").cloned().unwrap_or_default(),
                filled.get("output").cloned().unwrap_or_default(),
            )
        })
        .collect();
    let messages: Vec<Value> = turns
        .iter()
        .flat_map(|(person, agent)| {
            [
                json!({ "role": "user", "content": person }),
                json!({ "role": "assistant", "content": agent }),
            ]
        })
        .collect();
    let exchanges: Vec<Value> = turns
        .iter()
        .map(|(person, agent)| json!({ "person": person, "agent": agent }))
        .collect();
    let first_and_last = json!({
        "first_prompt": turns.first().map(|(person, _)| person.clone()).unwrap_or_default(),
        "last_answer": turns.last().map(|(_, agent)| agent.clone()).unwrap_or_default(),
    });
    let written = |value: &Value| serde_json::to_string_pretty(value).unwrap_or_default();
    BTreeMap::from([
        ("messages", bounded(written(&Value::Array(messages)))),
        ("exchanges", bounded(written(&Value::Array(exchanges)))),
        ("first_and_last", bounded(written(&first_and_last))),
    ])
}

/// Each variable `pointers` names narrowed to what its JSON Pointer finds:
/// a string as its text, anything else as JSON; nothing where the variable
/// is no JSON or the pointer finds nothing.
pub fn narrowed(
    mut filled: BTreeMap<&'static str, String>,
    pointers: &BTreeMap<String, String>,
) -> BTreeMap<&'static str, String> {
    for (variable, text) in &mut filled {
        let Some(pointer) = pointers.get(*variable) else {
            continue;
        };
        *text = match serde_json::from_str::<Value>(text)
            .ok()
            .as_ref()
            .and_then(|value| value.pointer(pointer))
        {
            Some(Value::String(found)) => found.clone(),
            Some(found) => serde_json::to_string_pretty(found).unwrap_or_default(),
            None => String::new(),
        };
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(score: ScoreSpec) -> Prompt {
        Prompt {
            messages: vec![
                Message {
                    role: PromptRole::System,
                    content: "You judge answers.".into(),
                },
                Message {
                    role: PromptRole::User,
                    content: "Q: {{ input }}\nA: {{output}}".into(),
                },
            ],
            score,
            reasoning: true,
        }
    }

    fn yes_or_no() -> ScoreSpec {
        ScoreSpec {
            name: " correct ".into(),
            data_type: ScoreType::Boolean,
            range_min: None,
            range_max: None,
            low_means: "wrong".into(),
            high_means: "right".into(),
            categories: Vec::new(),
            multiple: false,
        }
    }

    /// A pointer narrows a variable holding JSON to what it finds, and one
    /// that finds nothing leaves it empty.
    #[test]
    fn a_pointer_narrows_a_variable() {
        let filled = BTreeMap::from([
            ("input", r#"[{"role":"user","content":"hi"}]"#.to_owned()),
            ("output", "plain".to_owned()),
        ]);
        let pointers = BTreeMap::from([
            ("input".to_owned(), "/0/content".to_owned()),
            ("output".to_owned(), "/0".to_owned()),
        ]);
        let narrowed = narrowed(filled, &pointers);
        assert_eq!(narrowed["input"], "hi");
        assert_eq!(narrowed["output"], "");
    }

    /// Every template reads, holds to an evaluator's rules, and is named once.
    #[test]
    fn every_template_reads_and_holds() {
        assert_eq!(TEMPLATES.len(), TEMPLATE_FILES.len(), "each file reads");
        let keys: BTreeSet<&str> = TEMPLATES.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys.len(), TEMPLATES.len());
        for template in TEMPLATES.iter() {
            template
                .prompt
                .clone()
                .checked()
                .unwrap_or_else(|e| panic!("{}: {e}", template.key));
            assert!(
                ["quality", "safety", "security", "conversation", "agents"]
                    .contains(&template.group.as_str()),
                "{}",
                template.key
            );
        }
    }

    /// A typed evaluator judges on a System One connection alone, and a
    /// prompt on any other.
    #[test]
    fn each_kind_judges_on_its_own_protocol() {
        use telmoni_shared::ModelProvider;
        let typed = Judging::Typed(TYPED_TEMPLATES[0].typed.clone());
        let prompt = Judging::Prompt(TEMPLATES[0].prompt.clone());
        assert!(misfit(ModelProvider::SystemOneCompatible, &typed).is_none());
        assert!(misfit(ModelProvider::Openai, &typed).is_some());
        assert!(misfit(ModelProvider::Typesafe, &prompt).is_some());
        assert!(misfit(ModelProvider::Anthropic, &prompt).is_none());
    }

    /// Every typed template reads, holds to a typed evaluator's rules, and
    /// shares no key with another template.
    #[test]
    fn every_typed_template_reads_and_holds() {
        assert_eq!(
            TYPED_TEMPLATES.len(),
            TYPED_TEMPLATE_FILES.len(),
            "each file reads"
        );
        let keys: BTreeSet<&str> = TEMPLATES
            .iter()
            .map(|t| t.key.as_str())
            .chain(TYPED_TEMPLATES.iter().map(|t| t.key.as_str()))
            .collect();
        assert_eq!(keys.len(), TEMPLATES.len() + TYPED_TEMPLATES.len());
        for template in TYPED_TEMPLATES.iter() {
            template
                .typed
                .clone()
                .checked()
                .unwrap_or_else(|e| panic!("{}: {e}", template.key));
            assert!(
                ["quality", "safety", "security", "conversation", "agents"]
                    .contains(&template.group.as_str()),
                "{}",
                template.key
            );
        }
    }

    /// A prompt names known variables alone, a system message first, and a
    /// score whose every end is described.
    #[test]
    fn a_prompt_is_held_to_its_rules() {
        let kept = prompt(yes_or_no()).checked().expect("kept");
        assert_eq!(kept.score.name, "correct");
        assert_eq!(
            kept.variables().into_iter().collect::<Vec<_>>(),
            ["input", "output"]
        );
        let mut unknown = prompt(yes_or_no());
        unknown.messages[1].content = "{{answer}}".into();
        assert!(unknown.checked().is_err());
        let mut late = prompt(yes_or_no());
        late.messages.reverse();
        assert!(late.checked().is_err(), "the system message comes first");
        let mut vague = prompt(yes_or_no());
        vague.score.low_means = String::new();
        assert!(vague.checked().is_err());
        let ranged = ScoreSpec {
            data_type: ScoreType::Number,
            range_min: Some(1.0),
            range_max: Some(1.0),
            ..yes_or_no()
        };
        assert!(prompt(ranged).checked().is_err());
    }

    /// The prompt as sent fills each variable, describes the score after the
    /// system message, and leaves a brace pair that names none as written.
    #[test]
    fn the_prompt_as_sent_fills_its_variables() {
        let kept = prompt(ScoreSpec {
            data_type: ScoreType::Number,
            range_min: Some(0.0),
            range_max: Some(1.0),
            ..yes_or_no()
        })
        .checked()
        .expect("kept");
        let filled = BTreeMap::from([("input", "2+2?".to_owned()), ("output", "4".to_owned())]);
        let (system, messages) = kept.rendered(&filled);
        assert!(system.starts_with("You judge answers.\n\nThe score is \"correct\". It is a number from 0 to 1: 0 means wrong; 1 means right."), "{system}");
        assert_eq!(messages, [(Role::User, "Q: 2+2?\nA: 4".to_owned())]);
        assert_eq!(fill("{{ x }} and {{", &filled), "{{ x }} and {{");
    }

    /// An answer in the schema is the score and its reasoning; one outside
    /// it, a category not named or a number off the range, judges nothing.
    #[test]
    fn an_answer_is_read_against_the_scores_schema() {
        let kept = prompt(yes_or_no()).checked().expect("kept");
        let (judged, reasoning) = kept
            .judgement(&json!({ "reasoning": "It adds up.", "score": true }))
            .expect("judged");
        assert_eq!(
            (judged.value, reasoning.as_deref()),
            (Some(1.0), Some("It adds up."))
        );
        assert!(
            kept.judgement(&json!({ "score": true })).is_err(),
            "no reasoning"
        );

        let categories = prompt(ScoreSpec {
            data_type: ScoreType::Category,
            low_means: String::new(),
            high_means: String::new(),
            categories: vec![
                Category {
                    name: "calm".into(),
                    means: "even".into(),
                },
                Category {
                    name: "rude".into(),
                    means: "curt".into(),
                },
            ],
            ..yes_or_no()
        })
        .checked()
        .expect("kept");
        assert_eq!(
            categories.schema()["properties"]["score"]["enum"],
            json!(["calm", "rude"])
        );
        assert!(
            categories
                .judgement(&json!({ "reasoning": "r", "score": "loud" }))
                .is_err()
        );
        let (judged, _) = categories
            .judgement(&json!({ "reasoning": "r", "score": "calm" }))
            .expect("judged");
        assert_eq!(judged.categories, ["calm"]);

        let ranged = prompt(ScoreSpec {
            data_type: ScoreType::Number,
            range_min: Some(0.0),
            range_max: Some(1.0),
            ..yes_or_no()
        })
        .checked()
        .expect("kept");
        assert!(
            ranged
                .judgement(&json!({ "reasoning": "r", "score": 3 }))
                .is_err()
        );
    }
}
