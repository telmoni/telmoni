//! A project's evaluators: owners and admins save a version and delete one,
//! each filed in the chain, and try a draft on a real trace before saving;
//! every member reads them. A project that keeps no content has nothing for
//! an evaluator to read, so none is saved or tried there.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::types::Json as Jsonb;
use uuid::Uuid;

use telmoni_shared::acting::{Acting, ActingKey};
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit, lock_chain_in_time};
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::model::systemone::{Decide, Question};
use telmoni_shared::model::{self, Ask, Role};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, ContentMode, ModelProvider, ProjectId, TelmoniError,
    TelmoniResourceKind,
};

use crate::AppState;
use crate::evaluators::typed::Typed;
use crate::evaluators::{
    self, Judging, Message, NAME_MAX, Prompt, RUN_VARIABLES, ScoreSpec, TEMPLATES, TYPED_TEMPLATES,
    Template, TypedTemplate, VARIABLES, misfit,
};
use crate::handler::model_connections::{self, MODEL_MAX};
use crate::handler::{chain, content_kept};
use crate::scores::Checked;
use crate::store::{self, Projects};
use crate::{content, db, traces};

/// The most evaluators a project holds.
pub const EVALUATORS_MAX: i64 = 50;
/// The most an answer may run to, reasoning and score.
pub const ANSWER_TOKENS_MAX: u32 = 1_024;
/// The highest temperature an evaluator asks its model for.
pub const TEMPERATURE_MAX: f64 = 2.0;
/// The highest weekly spend limit, in dollars.
pub const WEEKLY_LIMIT_MAX: f64 = 1_000_000.0;
/// What a project keeping no content leaves an evaluator.
const EVALUATOR_READS: &str = "so an evaluator has nothing to read: switch content on first";
/// The most spans of a run a try reads.
const TRY_SPANS_MAX: u32 = 500;

/// One version of an evaluator, as the lanes answer it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Evaluator {
    pub id: Uuid,
    pub version: i32,
    pub name: String,
    pub kind: EvaluatorKind,
    /// A prompt one's messages and score; none on a typed one.
    pub messages: Option<Jsonb<Vec<Message>>>,
    pub score: Option<Jsonb<ScoreSpec>>,
    /// A typed one's state and questions; none on a prompt one.
    pub state: Option<String>,
    pub questions: Option<Jsonb<Vec<Question>>>,
    pub reasoning: bool,
    /// Whether a prompt one sends the files its variables name.
    pub media: bool,
    /// None follows the project's default connection.
    pub connection_id: Option<Uuid>,
    /// None is its connection's first model.
    pub model: Option<String>,
    pub temperature: Option<f64>,
    /// The most it spends a week, in US dollars; none for no limit.
    pub weekly_limit: Option<f64>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    /// How many versions it has; on its newest alone.
    #[sqlx(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versions: Option<i64>,
}

/// The project's evaluators, each its newest version, and whether the
/// project keeps the content they read.
#[derive(Debug, Serialize)]
pub struct Evaluators {
    pub evaluators: Vec<Evaluator>,
    pub content_kept: bool,
}

/// One evaluator's versions, its newest first.
#[derive(Debug, Serialize)]
pub struct Versions {
    pub versions: Vec<Evaluator>,
}

/// The templates to copy, prompt and typed.
#[derive(Debug, Serialize)]
pub struct Templates {
    pub templates: &'static [Template],
    pub typed: &'static [TypedTemplate],
}

/// What a save or a try sends: the evaluator's id where it changes one.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluatorChange {
    #[serde(default)]
    pub id: Option<Uuid>,
    pub name: String,
    #[serde(default)]
    pub kind: EvaluatorKind,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub score: Option<ScoreSpec>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub questions: Vec<Question>,
    #[serde(default = "yes")]
    pub reasoning: bool,
    #[serde(default)]
    pub media: bool,
    #[serde(default)]
    pub connection_id: Option<Uuid>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub weekly_limit: Option<f64>,
}

const fn yes() -> bool {
    true
}

/// The kinds of evaluator a change saves: a prompt to any model, or typed
/// questions to one speaking System One.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum EvaluatorKind {
    #[default]
    Prompt,
    Typed,
}

telmoni_shared::wire_enum!(EvaluatorKind {
    Prompt => "prompt",
    Typed => "typed",
});

/// A try: the draft, and the trace it judges.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trial {
    pub evaluator: EvaluatorChange,
    pub run_id: Uuid,
}

/// A try's answer: the prompt as sent, the score and reasoning it gave,
/// what it used; or why the model's answer judged nothing.
#[derive(Debug, Serialize)]
pub struct Tried {
    pub system: String,
    pub messages: Vec<Message>,
    pub model: String,
    pub judgement: Option<Checked>,
    /// A typed evaluator's scores, one a question, each with its answer.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<Answered>,
    pub reasoning: Option<String>,
    pub refused: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// One typed question's score, and its answer: probabilities, confidence, model.
#[derive(Debug, Serialize)]
pub struct Answered {
    #[serde(flatten)]
    pub judgement: Checked,
    pub answer: serde_json::Value,
}

/// A change held to an evaluator's rules, and what it asks to its kind's.
fn checked(mut change: EvaluatorChange) -> Result<(EvaluatorChange, Judging), TelmoniError> {
    let refuse = |why: &str| Err(TelmoniError::BadRequest(why.to_owned()));
    change.name = change.name.trim().to_owned();
    if change.name.is_empty() || change.name.chars().count() > NAME_MAX {
        return refuse("an evaluator's name is 1 to 60 characters");
    }
    change.model = change
        .model
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty());
    if change
        .model
        .as_ref()
        .is_some_and(|m| m.chars().count() > MODEL_MAX)
    {
        return refuse("a model's name is up to 128 characters");
    }
    if change
        .temperature
        .is_some_and(|t| !t.is_finite() || !(0.0..=TEMPERATURE_MAX).contains(&t))
    {
        return refuse("a temperature is from 0 to 2");
    }
    if change
        .weekly_limit
        .is_some_and(|l| !l.is_finite() || l <= 0.0 || l > WEEKLY_LIMIT_MAX)
    {
        return refuse("a weekly spend limit is more than $0, up to $1,000,000");
    }
    let judging = match change.kind {
        EvaluatorKind::Prompt => {
            if change.state.is_some() || !change.questions.is_empty() {
                return refuse("a prompt evaluator asks no typed questions");
            }
            let Some(score) = change.score.clone() else {
                return refuse("a prompt evaluator names the score it gives");
            };
            Judging::Prompt(
                Prompt {
                    messages: std::mem::take(&mut change.messages),
                    score,
                    reasoning: change.reasoning,
                }
                .checked()?,
            )
        }
        EvaluatorKind::Typed => {
            if !change.messages.is_empty() || change.score.is_some() {
                return refuse("a typed evaluator holds a state and questions, not a prompt");
            }
            if change.temperature.is_some() {
                return refuse("a typed evaluator takes no temperature");
            }
            if change.media {
                return refuse("a typed evaluator sends no files: its model reads text alone");
            }
            change.reasoning = false;
            Judging::Typed(
                Typed {
                    state: change.state.take().unwrap_or_default(),
                    questions: std::mem::take(&mut change.questions),
                }
                .checked()?,
            )
        }
    };
    Ok((change, judging))
}

/// The connection an evaluator judges with, opened, and its model: the one
/// it names, else the project's default; its own model, else the
/// connection's first. A typed evaluator judges on a System One connection
/// alone, and a prompt on any other.
async fn judge_with(
    state: &AppState,
    project_id: &ProjectId,
    change: &EvaluatorChange,
    judging: &Judging,
) -> Result<(model::Connection, String), TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    let row: Option<(Uuid, Vec<String>, ModelProvider)> = sqlx::query_as(
        "SELECT id, models, provider FROM telemetry.model_connections
          WHERE project_id = $1 AND (id = $2 OR ($2 IS NULL AND is_default))",
    )
    .bind(project_id)
    .bind(change.connection_id)
    .fetch_optional(tx.conn())
    .await?;
    tx.commit().await?;
    let Some((id, models, provider)) = row else {
        return Err(TelmoniError::BadRequest(match change.connection_id {
            Some(_) => "no model connection with that id in this project".into(),
            None => "an evaluator judges with a model connection: connect one on the project's Settings page first".into(),
        }));
    };
    if let Some(why) = misfit(provider, judging) {
        return Err(TelmoniError::BadRequest(why.into()));
    }
    let model = change
        .model
        .clone()
        .or_else(|| models.first().cloned())
        .unwrap_or_default();
    Ok((model_connections::open(state, project_id, id).await?, model))
}

/// `GET /internal/v1/telemetry/evaluators` — each evaluator's newest version
/// with its count, every member's to read.
pub async fn get_evaluators(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Evaluators>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Project)?
        .project_id
        .clone();
    Ok(Json(listed(&state, &project_id).await?))
}

/// The project's evaluators, each at its newest version, by name.
///
/// # Errors
///
/// Postgres did not answer.
pub async fn listed(state: &AppState, project_id: &ProjectId) -> Result<Evaluators, TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    let evaluators = sqlx::query_as(
        "SELECT DISTINCT ON (id) id, version, name, kind, messages, score, state, questions,
                reasoning, media, connection_id,
                model, temperature, weekly_limit, created_by, created_at,
                count(*) OVER (PARTITION BY id) AS versions
           FROM telemetry.evaluators WHERE project_id = $1
          ORDER BY id, version DESC",
    )
    .bind(project_id)
    .fetch_all(tx.conn())
    .await?;
    let mode = db::content_mode(&mut tx, project_id).await?;
    tx.commit().await?;
    let mut evaluators: Vec<Evaluator> = evaluators;
    evaluators.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Evaluators {
        evaluators,
        content_kept: mode == ContentMode::On,
    })
}

/// `GET /internal/v1/telemetry/evaluators/templates` — the library to copy.
pub async fn get_templates(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Templates>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    acting.require_project(Verb::Read, Resource::Project)?;
    Ok(Json(Templates {
        templates: TEMPLATES.as_slice(),
        typed: TYPED_TEMPLATES.as_slice(),
    }))
}

/// `GET /internal/v1/telemetry/evaluators/{id}` — one evaluator's versions,
/// its newest first.
pub async fn get_versions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Versions>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Project)?
        .project_id
        .clone();
    Ok(Json(versions_of(&state, &project_id, id).await?))
}

/// One evaluator's versions, its newest first.
///
/// # Errors
///
/// A 404 for an id the project holds no evaluator by, or Postgres did not answer.
pub async fn versions_of(
    state: &AppState,
    project_id: &ProjectId,
    id: Uuid,
) -> Result<Versions, TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    let versions: Vec<Evaluator> = sqlx::query_as(
        "SELECT id, version, name, kind, messages, score, state, questions, reasoning, media,
                connection_id, model, temperature, weekly_limit, created_by, created_at
           FROM telemetry.evaluators WHERE project_id = $1 AND id = $2
          ORDER BY version DESC",
    )
    .bind(project_id)
    .bind(id)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    if versions.is_empty() {
        return Err(AuthError::NotFound("no evaluator with that id in this project".into()).into());
    }
    Ok(Versions { versions })
}

/// `PUT /internal/v1/telemetry/evaluators` — a new evaluator, or a new
/// version of one by its id, by an owner or admin. Its model is asked once
/// for an answer in the score's schema first, and a model that cannot give
/// one saves nothing; then it is filed in the chain and written.
pub async fn put_evaluator(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(change): Json<EvaluatorChange>,
) -> Result<Json<Evaluator>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Project)?
        .project_id
        .clone();
    Ok(Json(
        saved(&state, Changer::Person(&acting), &project_id, change).await?,
    ))
}

/// Who changes an evaluator: an owner or admin on the console, or a project
/// key holding `evaluate` under `/v1`, each filed in the chain as itself.
pub enum Changer<'a> {
    Person(&'a Acting),
    Key(&'a ActingKey),
}

impl Changer<'_> {
    fn organization_id(&self) -> &telmoni_shared::OrganizationId {
        match self {
            Self::Person(acting) => &acting.organization_id,
            Self::Key(key) => &key.organization_id,
        }
    }

    /// Who the chain names, and the evaluator's `created_by`.
    fn who(&self) -> String {
        match self {
            Self::Person(acting) => acting.user_id.to_string(),
            Self::Key(_) => Actor::Key(&self.token()).to_string(),
        }
    }

    /// A key's row's id, as its chain row names it; empty for a person.
    fn token(&self) -> String {
        match self {
            Self::Person(_) => String::new(),
            Self::Key(key) => key.token_id.to_string(),
        }
    }
}

/// `work` under the organization's chain lock, as the changer may still do
/// it there: a person's role read again, a key's project still held by its
/// organization, so a move that passed before the lock files nothing.
async fn changing<T>(
    state: &AppState,
    changer: &Changer<'_>,
    project_id: &ProjectId,
    work: impl AsyncFnOnce(&mut Scoped<'_, ProjectAndOrganization>) -> Result<T, TelmoniError>,
) -> Result<T, TelmoniError> {
    match changer {
        Changer::Person(acting) => {
            let auth = &*state.auth;
            chain::write(&state.db, acting, project_id, "an evaluator", async |tx| {
                chain::held(auth, acting, tx, Verb::Update, Resource::Project).await?;
                work(tx).await
            })
            .await
        }
        Changer::Key(key) => {
            let tx = project_scope(&state.db, project_id)
                .await?
                .bind_organization(&key.organization_id)
                .await?;
            chain::settle(tx, project_id, "an evaluator", async |tx| {
                lock_chain_in_time(tx.conn(), &key.organization_id).await?;
                if db::settings_organization(tx, project_id)
                    .await?
                    .is_some_and(|holder| holder != key.organization_id)
                {
                    return Err(AuthError::NotFound(
                        "the key's project has moved to another organization".into(),
                    )
                    .into());
                }
                work(tx).await
            })
            .await
        }
    }
}

/// A new evaluator, or a new version of one by its id. Its model is asked
/// once for an answer in the score's schema first, and a model that cannot
/// give one saves nothing; then it is filed in the chain and written.
///
/// # Errors
///
/// A 400 for a change an evaluator's rules refuse, a model that cannot
/// answer, or a project that keeps no content; a 404 for a version of an id
/// the project holds none by; the chain or Postgres not had.
pub async fn saved(
    state: &AppState,
    changer: Changer<'_>,
    project_id: &ProjectId,
    change: EvaluatorChange,
) -> Result<Evaluator, TelmoniError> {
    let (change, judging) = checked(change)?;
    content_kept(
        db::content_mode_of(&state.db, project_id).await?,
        EVALUATOR_READS,
    )?;
    let (connection, model) = judge_with(state, project_id, &change, &judging).await?;
    let filled: BTreeMap<&str, String> = VARIABLES
        .iter()
        .map(|v| {
            (
                *v,
                "(left empty: this checks the answer's form alone)".to_owned(),
            )
        })
        .collect();
    tried(
        state,
        &connection,
        &model,
        &judging,
        change.temperature,
        (&filled, &[], 0),
    )
    .await
    .and_then(|tried| tried.refused.map_or(Ok(()), Err))
    .map_err(|why| TelmoniError::BadRequest(format!("the evaluator was not saved: {why}")))?;

    let id = change.id.unwrap_or_else(Uuid::new_v4);
    let fresh = change.id.is_none();
    let created_by = changer.who();
    let token = changer.token();
    let organization_id = changer.organization_id().clone();
    let written = changing(state, &changer, project_id, async |tx| {
        content_kept(db::content_mode(tx, project_id).await?, EVALUATOR_READS)?;
        let (count, last): (i64, Option<i32>) = sqlx::query_as(
            "SELECT count(DISTINCT id), max(version) FILTER (WHERE id = $2)
               FROM telemetry.evaluators WHERE project_id = $1",
        )
        .bind(project_id)
        .bind(id)
        .fetch_one(tx.conn())
        .await?;
        if !fresh && last.is_none() {
            return Err(
                AuthError::NotFound("no evaluator with that id in this project".into()).into(),
            );
        }
        if fresh && count >= EVALUATORS_MAX {
            return Err(TelmoniError::BadRequest(format!(
                "a project holds {EVALUATORS_MAX} evaluators at most"
            )));
        }
        let version = last.unwrap_or(0) + 1;
        let (kind, prompt, typed) = match &judging {
            Judging::Prompt(prompt) => (EvaluatorKind::Prompt, Some(prompt), None),
            Judging::Typed(typed) => (EvaluatorKind::Typed, None, Some(typed)),
        };
        let mut row: Evaluator = sqlx::query_as(
            "INSERT INTO telemetry.evaluators
                 (id, version, project_id, name, kind, messages, score, state, questions,
                  reasoning, media, connection_id, model, temperature, weekly_limit, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
             RETURNING id, version, name, kind, messages, score, state, questions, reasoning,
                       media, connection_id, model, temperature, weekly_limit, created_by,
                       created_at",
        )
        .bind(id)
        .bind(version)
        .bind(project_id)
        .bind(&change.name)
        .bind(kind)
        .bind(prompt.map(|p| Jsonb(&p.messages)))
        .bind(prompt.map(|p| Jsonb(&p.score)))
        .bind(typed.map(|t| t.state.as_str()))
        .bind(typed.map(|t| Jsonb(&t.questions)))
        .bind(prompt.is_some_and(|p| p.reasoning))
        .bind(change.media)
        .bind(change.connection_id)
        .bind(&change.model)
        .bind(change.temperature)
        .bind(change.weekly_limit)
        .bind(&created_by)
        .fetch_one(tx.conn())
        .await?;
        row.versions = Some(i64::from(version));
        let details = match &judging {
            Judging::Prompt(prompt) => json!({
                "version": version, "kind": kind, "score": prompt.score.name,
                "data_type": prompt.score.data_type, "connection_id": change.connection_id,
            }),
            Judging::Typed(typed) => json!({
                "version": version, "kind": kind,
                "questions": typed.questions.iter().map(|q| q.name.as_str()).collect::<Vec<_>>(),
                "connection_id": change.connection_id,
            }),
        };
        if fresh {
            emit_audit(
                tx,
                AuditEvent::new(
                    &organization_id,
                    actor_of(&changer, &token),
                    AuditAction::Created,
                    TelmoniResourceKind::Evaluator,
                )
                .in_project(project_id)
                .on(&id.to_string())
                .details(details),
            )
            .await?;
        } else {
            emit_audit(
                tx,
                AuditEvent::new(
                    &organization_id,
                    actor_of(&changer, &token),
                    AuditAction::Updated,
                    TelmoniResourceKind::Evaluator,
                )
                .in_project(project_id)
                .on(&id.to_string())
                .details(details),
            )
            .await?;
        }
        Ok(row)
    })
    .await?;
    tracing::info!(project_id = %project_id, evaluator_id = %id, version = written.version, "telemetry: an evaluator saved");
    Ok(written)
}

/// The chain's actor for `changer`, a key's by `token`, its row's id.
fn actor_of<'a>(changer: &'a Changer<'_>, token: &'a str) -> Actor<'a> {
    match changer {
        Changer::Person(acting) => Actor::User(acting.user_id.as_str()),
        Changer::Key(_) => Actor::Key(token),
    }
}

/// One judgement of what `judging` asks, filled with `filled`, by its kind.
async fn tried(
    state: &AppState,
    connection: &model::Connection,
    model: &str,
    judging: &Judging,
    temperature: Option<f64>,
    (filled, files, left): (&BTreeMap<&str, String>, &[model::Media], usize),
) -> Result<Tried, String> {
    match judging {
        Judging::Prompt(prompt) => {
            judged(
                state,
                connection,
                model,
                prompt,
                temperature,
                (filled, files, left),
            )
            .await
        }
        Judging::Typed(typed) => decided(state, connection, model, typed, filled).await,
    }
}

/// One decision of `typed`'s questions on its state filled with `filled`:
/// the state as sent, each question's score and answer or why it judged
/// nothing, and the tokens used. `Err` is a provider's refusal, named.
async fn decided(
    state: &AppState,
    connection: &model::Connection,
    model: &str,
    typed: &Typed,
    filled: &BTreeMap<&str, String>,
) -> Result<Tried, String> {
    let (sent, _) = typed.rendered(filled);
    let decide = Decide {
        model,
        state: &sent,
        questions: &typed.questions,
    };
    let decision = model::decide(&state.egress, connection, &decide)
        .await
        .map_err(|refused| refused.sentence())?;
    let (answers, refused) = match typed.judgements(&decision) {
        Ok(read) => (
            read.into_iter()
                .map(|(judgement, answer)| Answered { judgement, answer })
                .collect(),
            None,
        ),
        Err(why) => (Vec::new(), Some(why)),
    };
    Ok(Tried {
        system: String::new(),
        messages: vec![Message {
            role: evaluators::PromptRole::User,
            content: sent,
        }],
        model: decision.model,
        judgement: None,
        answers,
        reasoning: None,
        refused,
        input_tokens: decision.usage.input_tokens,
        output_tokens: decision.usage.output_tokens,
    })
}

/// One judgement of `prompt` filled with `filled`: the prompt as sent, the
/// score read from the answer or why it judged nothing, and the tokens used.
/// `Err` is a provider's refusal, named.
async fn judged(
    state: &AppState,
    connection: &model::Connection,
    model: &str,
    prompt: &Prompt,
    temperature: Option<f64>,
    (filled, files, left): (&BTreeMap<&str, String>, &[model::Media], usize),
) -> Result<Tried, String> {
    let (system, mut messages) = prompt.rendered(filled);
    evaluators::noted(&mut messages, left);
    let schema = prompt.schema();
    let ask = Ask {
        model,
        system: &system,
        messages: &messages,
        schema: &schema,
        temperature,
        max_tokens: ANSWER_TOKENS_MAX,
        media: files,
    };
    let answer = model::judge(&state.egress, connection, &ask)
        .await
        .map_err(|refused| refused.sentence())?;
    let (judgement, reasoning, refused) = match prompt.judgement(&answer.answer) {
        Ok((judgement, reasoning)) => (Some(judgement), reasoning, None),
        Err(why) => (None, None, Some(why)),
    };
    Ok(Tried {
        system,
        messages: messages
            .into_iter()
            .map(|(role, content)| Message {
                role: match role {
                    Role::User => evaluators::PromptRole::User,
                    Role::Assistant => evaluators::PromptRole::Assistant,
                },
                content,
            })
            .collect(),
        model: model.to_owned(),
        judgement,
        answers: Vec::new(),
        reasoning,
        refused,
        input_tokens: answer.usage.input_tokens,
        output_tokens: answer.usage.output_tokens,
    })
}

/// `POST /internal/v1/telemetry/evaluators/try` — a draft judged on one of
/// the project's traces, by an owner or admin: its content opened and filed
/// as their read, the prompt as sent, the score and its reasoning answered,
/// nothing kept. A prompt naming a session's variables is tried on a session
/// by its rule, never on a trace.
pub async fn try_evaluator(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(trial): Json<Trial>,
) -> Result<Json<Tried>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Project)?
        .project_id
        .clone();
    let (change, judging) = checked(trial.evaluator)?;
    if let Some(other) = judging
        .variables()
        .into_iter()
        .find(|v| !RUN_VARIABLES.contains(&v.as_str()))
    {
        return Err(TelmoniError::BadRequest(format!(
            "{{{{{other}}}}} is no trace's to fill, so this draft is not tried on a trace"
        )));
    }
    content_kept(
        db::content_mode_of(&state.db, &project_id).await?,
        EVALUATOR_READS,
    )?;
    let (connection, model) = judge_with(&state, &project_id, &change, &judging).await?;
    let projects = Projects::one(project_id.clone());
    let spans = traces::spans(
        &state.store,
        &projects,
        trial.run_id,
        TRY_SPANS_MAX,
        u32::MAX,
    )
    .await
    .map_err(|e| {
        store::failed(
            &e,
            "a trace to try an evaluator on",
            "the span store did not answer; try again",
        )
    })?;
    // As the trace's page: a run past the organization's retention is not there.
    let reach = crate::capacity::visible_days(&state, &project_id, u16::MAX).await;
    let past = reach < u16::MAX
        && spans.iter().all(|row| {
            row.received_at < chrono::Utc::now() - chrono::Duration::days(i64::from(reach))
        });
    if spans.is_empty() || past {
        return Err(AuthError::NotFound("no trace with that run id in this project".into()).into());
    }
    let opened = content::open(
        &state,
        Actor::User(acting.user_id.as_str()),
        &project_id,
        &[trial.run_id],
        None,
    )
    .await?;
    let filled = evaluators::of_run(&spans, &opened);
    // The files its variables name, each opened as the member's read, as the
    // trace's page opens them.
    let (files, left) = match &judging {
        Judging::Prompt(prompt) if change.media => {
            let named = prompt.variables();
            evaluators::files_of(
                &state,
                Some(Actor::User(acting.user_id.as_str())),
                &project_id,
                filled
                    .iter()
                    .filter(|(variable, _)| named.contains(**variable))
                    .map(|(_, text)| text.as_str()),
                connection.provider,
            )
            .await?
        }
        _ => (Vec::new(), 0),
    };
    let tried = tried(
        &state,
        &connection,
        &model,
        &judging,
        change.temperature,
        (&filled, &files, left),
    )
    .await
    .map_err(|why| TelmoniError::BadRequest(format!("the model did not judge: {why}")))?;
    tracing::info!(project_id = %project_id, run_id = %trial.run_id, "telemetry: an evaluator tried");
    Ok(Json(tried))
}

/// One evaluation as an evaluator's page lists it: what it judged, where it
/// stands, and what it read and spent.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct EvaluationRow {
    pub id: Uuid,
    pub evaluator_version: i32,
    pub rule_id: Option<Uuid>,
    pub target: telmoni_shared::EvaluationTarget,
    pub run_id: Option<Uuid>,
    pub span_id: Option<String>,
    pub session_id: Option<String>,
    pub status: telmoni_shared::EvaluationStatus,
    pub attempts: i16,
    pub reason: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub read_at: Option<DateTime<Utc>>,
    pub cut: bool,
    pub score_id: Option<Uuid>,
    pub trace_run_id: Option<Uuid>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub settled_at: Option<DateTime<Utc>>,
}

/// An evaluator's newest evaluations, its counts by where each stands, and
/// what it spent since Monday 00:00 UTC.
#[derive(Debug, Serialize)]
pub struct Evaluations {
    pub evaluations: Vec<EvaluationRow>,
    pub pending: i64,
    pub delayed: i64,
    pub done: i64,
    pub failed: i64,
    pub spent_this_week: f64,
}

/// The newest an evaluator's page lists.
const EVALUATIONS_SHOWN: i64 = 50;

/// `GET /internal/v1/telemetry/evaluators/{id}/evaluations` — an
/// evaluator's newest evaluations and its counts, every member's to read.
pub async fn get_evaluations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Evaluations>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Project)?
        .project_id
        .clone();
    let mut tx = project_scope(&state.db, &project_id).await?;
    let evaluations = sqlx::query_as(
        "SELECT id, evaluator_version, rule_id, target, run_id, span_id, session_id, status,
                attempts, reason, provider, model, read_at, cut, score_id, trace_run_id,
                input_tokens, output_tokens, cost, created_at, settled_at
           FROM telemetry.evaluations WHERE project_id = $1 AND evaluator_id = $2
          ORDER BY created_at DESC LIMIT $3",
    )
    .bind(&project_id)
    .bind(id)
    .bind(EVALUATIONS_SHOWN)
    .fetch_all(tx.conn())
    .await?;
    let (pending, delayed, done, failed, spent): (i64, i64, i64, i64, Option<f64>) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'pending'), count(*) FILTER (WHERE status = 'delayed'),
                count(*) FILTER (WHERE status = 'done'), count(*) FILTER (WHERE status = 'failed'),
                sum(cost) FILTER (WHERE settled_at >= date_trunc('week', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
           FROM telemetry.evaluations WHERE project_id = $1 AND evaluator_id = $2",
    )
    .bind(&project_id)
    .bind(id)
    .fetch_one(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(Json(Evaluations {
        evaluations,
        pending,
        delayed,
        done,
        failed,
        spent_this_week: spent.unwrap_or(0.0),
    }))
}

/// What an agreement's reading takes: its window, in days.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgreementAsked {
    #[serde(default)]
    pub days: Option<u16>,
}

/// `GET /internal/v1/telemetry/evaluators/{id}/agreement?days=` — how far
/// the evaluator agrees with the project's members over the window, a score
/// name at a time, every member's to read: names and numbers alone, never a
/// score's comment.
pub async fn get_agreement(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    telmoni_shared::extract::Query(asked): telmoni_shared::extract::Query<AgreementAsked>,
) -> Result<Json<evaluators::agreement::Agreements>, TelmoniError> {
    use evaluators::agreement::{DAYS_DEFAULT, DAYS_MAX, agreements, pairs};
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Project)?
        .project_id
        .clone();
    let days = asked.days.unwrap_or(DAYS_DEFAULT);
    if !(1..=DAYS_MAX).contains(&days) {
        return Err(TelmoniError::BadRequest(format!(
            "an agreement is read over 1 to {DAYS_MAX} days"
        )));
    }
    let read = pairs(&state.store, &Projects::one(project_id), id, days)
        .await
        .map_err(|e| {
            store::failed(
                &e,
                "an evaluator's agreement",
                "the span store did not answer; try again",
            )
        })?;
    Ok(Json(agreements(days, &read)))
}

/// `DELETE /internal/v1/telemetry/evaluators/{id}` — an evaluator and every
/// version gone, by an owner or admin, filed in the chain. The scores it gave
/// stay, naming it and their version.
pub async fn delete_evaluator(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Project)?
        .project_id
        .clone();
    removed(&state, Changer::Person(&acting), &project_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// An evaluator gone, every version, its rules judging on with the rest.
///
/// # Errors
///
/// A 404 for an id the project holds no evaluator by; the chain or Postgres not had.
pub async fn removed(
    state: &AppState,
    changer: Changer<'_>,
    project_id: &ProjectId,
    id: Uuid,
) -> Result<(), TelmoniError> {
    let token = changer.token();
    let organization_id = changer.organization_id().clone();
    // The corrections it was taught first, as the content they are: a retry
    // after a failure finds the evaluator still there to delete.
    content::forget_prefixed(
        state,
        project_id,
        evaluators::examples::FIELD,
        &evaluators::examples::prefix_of(id),
    )
    .await?;
    changing(state, &changer, project_id, async |tx| {
        let versions =
            sqlx::query("DELETE FROM telemetry.evaluators WHERE project_id = $1 AND id = $2")
                .bind(project_id)
                .bind(id)
                .execute(tx.conn())
                .await?
                .rows_affected();
        if versions == 0 {
            return Err(
                AuthError::NotFound("no evaluator with that id in this project".into()).into(),
            );
        }
        // Its rules stay, judging with the evaluators they still name.
        sqlx::query(
            "DELETE FROM telemetry.evaluation_rule_evaluators
              WHERE project_id = $1 AND evaluator_id = $2",
        )
        .bind(project_id)
        .bind(id)
        .execute(tx.conn())
        .await?;
        emit_audit(
            tx,
            AuditEvent::new(
                &organization_id,
                actor_of(&changer, &token),
                AuditAction::Deleted,
                TelmoniResourceKind::Evaluator,
            )
            .in_project(project_id)
            .on(&id.to_string())
            .details(json!({ "versions": versions })),
        )
        .await?;
        Ok(())
    })
    .await?;
    tracing::info!(project_id = %project_id, evaluator_id = %id, "telemetry: an evaluator deleted");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A change keeps its name and model trimmed, and a temperature within 0 to 2.
    #[test]
    fn a_change_is_held_to_an_evaluators_rules() {
        let template = TEMPLATES.first().expect("a template").clone();
        let change = |temperature: Option<f64>| EvaluatorChange {
            id: None,
            name: format!(" {} ", template.name),
            kind: EvaluatorKind::Prompt,
            messages: template.prompt.messages.clone(),
            score: Some(template.prompt.score.clone()),
            state: None,
            questions: Vec::new(),
            reasoning: true,
            media: false,
            connection_id: None,
            model: Some("  ".into()),
            temperature,
            weekly_limit: None,
        };
        let (kept, judging) = checked(change(Some(0.0))).expect("kept");
        assert_eq!(
            (kept.name.as_str(), kept.model),
            (template.name.as_str(), None)
        );
        assert!(matches!(judging, Judging::Prompt(prompt) if prompt.messages.len() == 2));
        assert!(checked(change(Some(2.5))).is_err());
        assert!(
            checked(EvaluatorChange {
                score: None,
                ..change(None)
            })
            .is_err(),
            "a prompt names its score"
        );
    }

    /// A typed change holds a state and questions alone, never a prompt or a
    /// temperature, and is held to a typed evaluator's rules.
    #[test]
    fn a_typed_change_holds_a_state_and_questions() {
        let template = TYPED_TEMPLATES.first().expect("a typed template").clone();
        let change = || EvaluatorChange {
            id: None,
            name: template.name.clone(),
            kind: EvaluatorKind::Typed,
            messages: Vec::new(),
            score: None,
            state: Some(template.typed.state.clone()),
            questions: template.typed.questions.clone(),
            reasoning: true,
            media: false,
            connection_id: None,
            model: None,
            temperature: None,
            weekly_limit: None,
        };
        let (kept, judging) = checked(change()).expect("kept");
        assert!(!kept.reasoning, "a typed evaluator writes no reasoning");
        assert!(matches!(judging, Judging::Typed(_)));
        assert!(
            checked(EvaluatorChange {
                temperature: Some(0.0),
                ..change()
            })
            .is_err()
        );
        assert!(
            checked(EvaluatorChange {
                score: Some(TEMPLATES[0].prompt.score.clone()),
                ..change()
            })
            .is_err()
        );
        assert!(
            checked(EvaluatorChange {
                questions: Vec::new(),
                ..change()
            })
            .is_err()
        );
        assert!(
            checked(EvaluatorChange {
                media: true,
                ..change()
            })
            .is_err(),
            "a typed evaluator sends no files"
        );
    }
}
