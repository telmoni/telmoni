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

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::model::{self, Ask, Role};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, ContentMode, ProjectId, TelmoniError, TelmoniResourceKind,
};

use crate::AppState;
use crate::evaluators::{
    self, Message, NAME_MAX, Prompt, RUN_VARIABLES, ScoreSpec, TEMPLATES, Template, VARIABLES,
};
use crate::handler::chain;
use crate::handler::model_connections::{self, MODEL_MAX};
use crate::scores::Judgement;
use crate::store::{self, Projects};
use crate::{content, db, traces};

/// The most evaluators a project holds.
pub const EVALUATORS_MAX: i64 = 50;
/// The most an answer may run to, reasoning and score.
pub const ANSWER_TOKENS_MAX: u32 = 1_024;
/// The most spans of a run a try reads.
const TRY_SPANS_MAX: u32 = 500;

/// One version of an evaluator, as the lanes answer it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Evaluator {
    pub id: Uuid,
    pub version: i32,
    pub name: String,
    pub kind: String,
    pub messages: Jsonb<Vec<Message>>,
    pub score: Jsonb<ScoreSpec>,
    pub reasoning: bool,
    /// None follows the project's default connection.
    pub connection_id: Option<Uuid>,
    /// None is its connection's first model.
    pub model: Option<String>,
    pub temperature: Option<f64>,
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

/// The templates to copy.
#[derive(Debug, Serialize)]
pub struct Templates {
    pub templates: &'static [Template],
}

/// What a save or a try sends: the evaluator's id where it changes one.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluatorChange {
    #[serde(default)]
    pub id: Option<Uuid>,
    pub name: String,
    pub messages: Vec<Message>,
    pub score: ScoreSpec,
    #[serde(default = "yes")]
    pub reasoning: bool,
    #[serde(default)]
    pub connection_id: Option<Uuid>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
}

const fn yes() -> bool {
    true
}

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
    pub judgement: Option<Judgement>,
    pub reasoning: Option<String>,
    pub refused: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A change held to an evaluator's rules, its prompt to a prompt's.
fn checked(mut change: EvaluatorChange) -> Result<(EvaluatorChange, Prompt), TelmoniError> {
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
        .is_some_and(|t| !t.is_finite() || !(0.0..=2.0).contains(&t))
    {
        return refuse("a temperature is from 0 to 2");
    }
    let prompt = Prompt {
        messages: std::mem::take(&mut change.messages),
        score: change.score.clone(),
        reasoning: change.reasoning,
    }
    .checked()?;
    Ok((change, prompt))
}

/// The connection an evaluator judges with, opened, and its model: the one
/// it names, else the project's default; its own model, else the
/// connection's first.
async fn judge_with(
    state: &AppState,
    project_id: &ProjectId,
    change: &EvaluatorChange,
) -> Result<(model::Connection, String), TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    let row: Option<(Uuid, Vec<String>)> = sqlx::query_as(
        "SELECT id, models FROM telemetry.model_connections
          WHERE project_id = $1 AND (id = $2 OR ($2 IS NULL AND is_default))",
    )
    .bind(project_id)
    .bind(change.connection_id)
    .fetch_optional(tx.conn())
    .await?;
    tx.commit().await?;
    let Some((id, models)) = row else {
        return Err(TelmoniError::BadRequest(match change.connection_id {
            Some(_) => "no model connection with that id in this project".into(),
            None => "an evaluator judges with a model connection: connect one on the project's Settings page first".into(),
        }));
    };
    let model = change
        .model
        .clone()
        .or_else(|| models.first().cloned())
        .unwrap_or_default();
    Ok((model_connections::open(state, project_id, id).await?, model))
}

/// Refused where the project keeps no content: nothing for an evaluator to read.
fn content_kept(mode: ContentMode) -> Result<(), TelmoniError> {
    if mode == ContentMode::On {
        Ok(())
    } else {
        Err(TelmoniError::BadRequest(
            "this project keeps no content, so an evaluator has nothing to read: switch content on first".into(),
        ))
    }
}

async fn mode_of(state: &AppState, project_id: &ProjectId) -> Result<ContentMode, TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    let mode = db::content_mode(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(mode)
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
    let mut tx = project_scope(&state.db, &project_id).await?;
    let evaluators = sqlx::query_as(
        "SELECT DISTINCT ON (id) id, version, name, kind, messages, score, reasoning, connection_id,
                model, temperature, created_by, created_at,
                count(*) OVER (PARTITION BY id) AS versions
           FROM telemetry.evaluators WHERE project_id = $1
          ORDER BY id, version DESC",
    )
    .bind(&project_id)
    .fetch_all(tx.conn())
    .await?;
    let mode = db::content_mode(&mut tx, &project_id).await?;
    tx.commit().await?;
    let mut evaluators: Vec<Evaluator> = evaluators;
    evaluators.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(Evaluators {
        evaluators,
        content_kept: mode == ContentMode::On,
    }))
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
    let mut tx = project_scope(&state.db, &project_id).await?;
    let versions: Vec<Evaluator> = sqlx::query_as(
        "SELECT id, version, name, kind, messages, score, reasoning, connection_id, model,
                temperature, created_by, created_at
           FROM telemetry.evaluators WHERE project_id = $1 AND id = $2
          ORDER BY version DESC",
    )
    .bind(&project_id)
    .bind(id)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    if versions.is_empty() {
        return Err(AuthError::NotFound("no evaluator with that id in this project".into()).into());
    }
    Ok(Json(Versions { versions }))
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
    let (change, prompt) = checked(change)?;
    content_kept(mode_of(&state, &project_id).await?)?;
    let (connection, model) = judge_with(&state, &project_id, &change).await?;
    let filled: BTreeMap<&str, String> = VARIABLES
        .iter()
        .map(|v| {
            (
                *v,
                "(left empty: this checks the answer's form alone)".to_owned(),
            )
        })
        .collect();
    judged(
        &state,
        &connection,
        &model,
        &prompt,
        change.temperature,
        &filled,
    )
    .await
    .and_then(|tried| tried.refused.map_or(Ok(()), Err))
    .map_err(|why| TelmoniError::BadRequest(format!("the evaluator was not saved: {why}")))?;

    let id = change.id.unwrap_or_else(Uuid::new_v4);
    let fresh = change.id.is_none();
    let auth = &*state.auth;
    let written = chain::write(
        &state.db,
        &acting,
        &project_id,
        "an evaluator",
        async |tx| {
            chain::held(auth, &acting, tx, Verb::Update, Resource::Project).await?;
            content_kept(db::content_mode(tx, &project_id).await?)?;
            let (count, last): (i64, Option<i32>) = sqlx::query_as(
                "SELECT count(DISTINCT id), max(version) FILTER (WHERE id = $2)
               FROM telemetry.evaluators WHERE project_id = $1",
            )
            .bind(&project_id)
            .bind(id)
            .fetch_one(tx.conn())
            .await?;
            if !fresh && last.is_none() {
                return Err(AuthError::NotFound(
                    "no evaluator with that id in this project".into(),
                )
                .into());
            }
            if fresh && count >= EVALUATORS_MAX {
                return Err(TelmoniError::BadRequest(format!(
                    "a project holds {EVALUATORS_MAX} evaluators at most"
                )));
            }
            let version = last.unwrap_or(0) + 1;
            let mut row: Evaluator = sqlx::query_as(
                "INSERT INTO telemetry.evaluators
                 (id, version, project_id, name, messages, score, reasoning, connection_id, model,
                  temperature, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             RETURNING id, version, name, kind, messages, score, reasoning, connection_id, model,
                       temperature, created_by, created_at",
            )
            .bind(id)
            .bind(version)
            .bind(&project_id)
            .bind(&change.name)
            .bind(Jsonb(&prompt.messages))
            .bind(Jsonb(&prompt.score))
            .bind(prompt.reasoning)
            .bind(change.connection_id)
            .bind(&change.model)
            .bind(change.temperature)
            .bind(acting.user_id.as_str())
            .fetch_one(tx.conn())
            .await?;
            row.versions = Some(i64::from(version));
            let details = json!({
                "version": version, "score": prompt.score.name, "data_type": prompt.score.data_type,
                "connection_id": change.connection_id,
            });
            if fresh {
                emit_audit(
                    tx,
                    AuditEvent::new(
                        &acting.organization_id,
                        Actor::User(acting.user_id.as_str()),
                        AuditAction::Created,
                        TelmoniResourceKind::Evaluator,
                    )
                    .in_project(&project_id)
                    .on(&id.to_string())
                    .details(details),
                )
                .await?;
            } else {
                emit_audit(
                    tx,
                    AuditEvent::new(
                        &acting.organization_id,
                        Actor::User(acting.user_id.as_str()),
                        AuditAction::Updated,
                        TelmoniResourceKind::Evaluator,
                    )
                    .in_project(&project_id)
                    .on(&id.to_string())
                    .details(details),
                )
                .await?;
            }
            Ok(row)
        },
    )
    .await?;
    tracing::info!(project_id = %project_id, evaluator_id = %id, version = written.version, "telemetry: an evaluator saved");
    Ok(Json(written))
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
    filled: &BTreeMap<&str, String>,
) -> Result<Tried, String> {
    let (system, messages) = prompt.rendered(filled);
    let schema = prompt.schema();
    let ask = Ask {
        model,
        system: &system,
        messages: &messages,
        schema: &schema,
        temperature,
        max_tokens: ANSWER_TOKENS_MAX,
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
    let (change, prompt) = checked(trial.evaluator)?;
    if let Some(other) = prompt
        .variables()
        .into_iter()
        .find(|v| !RUN_VARIABLES.contains(&v.as_str()))
    {
        return Err(TelmoniError::BadRequest(format!(
            "{{{{{other}}}}} is no trace's to fill, so this draft is not tried on a trace"
        )));
    }
    content_kept(mode_of(&state, &project_id).await?)?;
    let (connection, model) = judge_with(&state, &project_id, &change).await?;
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
    let tried = judged(
        &state,
        &connection,
        &model,
        &prompt,
        change.temperature,
        &filled,
    )
    .await
    .map_err(|why| TelmoniError::BadRequest(format!("the model did not judge: {why}")))?;
    tracing::info!(project_id = %project_id, run_id = %trial.run_id, "telemetry: an evaluator tried");
    Ok(Json(tried))
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
    let auth = &*state.auth;
    chain::write(
        &state.db,
        &acting,
        &project_id,
        "an evaluator",
        async |tx| {
            chain::held(auth, &acting, tx, Verb::Update, Resource::Project).await?;
            let versions =
                sqlx::query("DELETE FROM telemetry.evaluators WHERE project_id = $1 AND id = $2")
                    .bind(&project_id)
                    .bind(id)
                    .execute(tx.conn())
                    .await?
                    .rows_affected();
            if versions == 0 {
                return Err(AuthError::NotFound(
                    "no evaluator with that id in this project".into(),
                )
                .into());
            }
            // Its rules stay, judging with the evaluators they still name.
            sqlx::query(
                "DELETE FROM telemetry.evaluation_rule_evaluators
                  WHERE project_id = $1 AND evaluator_id = $2",
            )
            .bind(&project_id)
            .bind(id)
            .execute(tx.conn())
            .await?;
            emit_audit(
                tx,
                AuditEvent::new(
                    &acting.organization_id,
                    Actor::User(acting.user_id.as_str()),
                    AuditAction::Deleted,
                    TelmoniResourceKind::Evaluator,
                )
                .in_project(&project_id)
                .on(&id.to_string())
                .details(json!({ "versions": versions })),
            )
            .await?;
            Ok(())
        },
    )
    .await?;
    tracing::info!(project_id = %project_id, evaluator_id = %id, "telemetry: an evaluator deleted");
    Ok(StatusCode::NO_CONTENT)
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
            messages: template.prompt.messages.clone(),
            score: template.prompt.score.clone(),
            reasoning: true,
            connection_id: None,
            model: Some("  ".into()),
            temperature,
        };
        let (kept, prompt) = checked(change(Some(0.0))).expect("kept");
        assert_eq!(
            (kept.name.as_str(), kept.model),
            (template.name.as_str(), None)
        );
        assert_eq!(prompt.messages.len(), 2);
        assert!(checked(change(Some(2.5))).is_err());
    }
}
