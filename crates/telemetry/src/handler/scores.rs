//! A member's scores: given on a trace's page, of the run, one of its spans
//! or its session, filed in the organization's chain; and every score of a
//! run read back, the application's, its members' and its evaluators'.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use chrono::Utc;
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, ScoreSource, ScoreTarget, TelmoniError, TelmoniResourceKind,
};

use crate::AppState;
use crate::handler::chain::{self, Change};
use crate::handler::traces::read_failed;
use crate::scores::{Judgement, RUN_SQL, ScoreRow, TargetKey, member_score_id};
use crate::store::Projects;

/// The longest session id a score names, as a span carries one.
const SESSION_MAX: usize = 200;

/// What a member's score sends: what it scores — a run, one of its spans or a
/// session — and what it says.
#[derive(Debug, Deserialize)]
pub struct ScoreChange {
    #[serde(default)]
    pub run_id: Option<Uuid>,
    #[serde(default)]
    pub span_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(flatten)]
    pub judgement: Judgement,
}

/// A score as the lanes answer it.
#[derive(Debug, Serialize)]
pub struct Score {
    pub score_id: Uuid,
    pub target: ScoreTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Uuid>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub span_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(flatten)]
    pub judgement: Judgement,
    pub source: ScoreSource,
    /// The member's user id or the evaluator's id; empty for the application's.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub scorer: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub scorer_version: String,
    pub given_at: chrono::DateTime<Utc>,
}

impl From<ScoreRow> for Score {
    fn from(row: ScoreRow) -> Self {
        Self {
            score_id: row.score_id,
            target: row.target,
            run_id: (!row.run_id.is_nil()).then_some(row.run_id),
            span_id: row.span_id,
            session_id: row.session_id,
            judgement: Judgement {
                value: (row.data_type != telmoni_shared::ScoreType::Category).then_some(row.value),
                name: row.name,
                data_type: row.data_type,
                categories: row.categories,
                range_min: row.range_min,
                range_max: row.range_max,
            },
            source: row.source,
            scorer: row.scorer,
            scorer_version: row.scorer_version,
            given_at: row.received_at,
        }
    }
}

/// A run's scores, as `GET /internal/v1/telemetry/scores` answers them.
#[derive(Debug, Serialize)]
pub struct Scores {
    pub scores: Vec<Score>,
}

/// Whether what a score names is the project's: its run, the span in it, or
/// a run of its session.
const EXISTS_SQL: &str = "SELECT count() AS n FROM telemetry.spans
     WHERE project_id IN {projects:Array(String)}
       AND (({session:String} = '' AND run_id = {run:UUID}
             AND ({span:String} = '' OR span_id = {span:String}))
         OR ({session:String} != '' AND session_id = {session:String}))";

/// The run whose scores are read, and its session's.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoresAsked {
    pub run_id: Uuid,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Row, Deserialize)]
struct Count {
    n: u64,
}

/// `PUT /internal/v1/telemetry/scores` — a member's score of a run, one of
/// its spans or a session, given again replacing their last under the same
/// name: filed in the organization's chain, naming its target and its name,
/// never its value, then written. `404` for a target the project does not hold.
pub async fn put_score(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(change): Json<ScoreChange>,
) -> Result<Json<Score>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Trace)?
        .project_id
        .clone();
    let judgement = change.judgement.checked()?;
    let session = change
        .session_id
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let span = change
        .span_id
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let target = match (change.run_id, span, session) {
        (Some(run), None, None) => TargetKey::Run(run),
        (Some(run), Some(span), None) => TargetKey::Span(run, span),
        (None, None, Some(session)) if session.chars().count() <= SESSION_MAX => {
            TargetKey::Session(session)
        }
        _ => {
            return Err(TelmoniError::BadRequest(
                "a score names a run, a run and one of its spans, or a session".into(),
            ));
        }
    };
    let (run, span, session) = match &target {
        TargetKey::Run(run) => (*run, String::new(), String::new()),
        TargetKey::Span(run, span) => (*run, span.clone(), String::new()),
        TargetKey::Session(session) => (Uuid::nil(), String::new(), session.clone()),
    };
    let projects = Projects::one(project_id.clone());
    let found: Count = state
        .store
        .read(&projects, EXISTS_SQL)
        .param("run", run)
        .param("span", span.as_str())
        .param("session", session.as_str())
        .fetch_one()
        .await
        .map_err(read_failed)?;
    if found.n == 0 {
        return Err(
            AuthError::NotFound("nothing with those ids in this project to score".into()).into(),
        );
    }
    let score_id = member_score_id(
        &project_id,
        &target,
        &judgement.name,
        acting.user_id.as_str(),
    );
    let row = ScoreRow {
        project_id: project_id.clone(),
        received_at: Utc::now(),
        score_id,
        target: match target {
            TargetKey::Run(_) => ScoreTarget::Run,
            TargetKey::Span(..) => ScoreTarget::Span,
            TargetKey::Session(_) => ScoreTarget::Session,
        },
        run_id: run,
        trace_id: String::new(),
        span_id: span,
        session_id: session,
        value: judgement.stored_value(),
        name: judgement.name.clone(),
        data_type: judgement.data_type,
        categories: judgement.categories.clone(),
        range_min: judgement.range_min,
        range_max: judgement.range_max,
        source: ScoreSource::Member,
        scorer: acting.user_id.to_string(),
        scorer_version: String::new(),
        is_deleted: 0,
    };
    let filed = row.clone();
    chain::write(&state.db, &acting, &project_id, "a score", async |tx| {
        chain::held(
            state.auth.as_ref(),
            &acting,
            tx,
            Verb::Update,
            Resource::Trace,
        )
        .await?;
        chain::file(
            tx,
            &acting,
            &project_id,
            Change {
                action: AuditAction::Updated,
                kind: TelmoniResourceKind::Trace,
                id: if filed.run_id.is_nil() {
                    filed.score_id
                } else {
                    filed.run_id
                },
                metadata: json!({ "score": {
                    "name": filed.name, "target": filed.target, "data_type": filed.data_type,
                } }),
            },
        )
        .await?;
        state
            .store
            .insert_scores_once(std::slice::from_ref(&filed), &Uuid::new_v4().to_string())
            .await
            .map_err(|error| {
                crate::store::failed(
                    &error,
                    "a score",
                    "the span store did not take the score; retry",
                )
            })
    })
    .await?;
    tracing::info!(project_id = %project_id, score_id = %score_id, target = %row.target, "telemetry: a member scored");
    Ok(Json(Score::from(row)))
}

/// `DELETE /internal/v1/telemetry/scores/{score_id}` — a member's score taken
/// back, by the member who gave it or an owner or admin, filed as it was given.
pub async fn delete_score(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(score_id): Path<Uuid>,
) -> Result<axum::http::StatusCode, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project = acting.require_project(Verb::Update, Resource::Trace)?;
    let project_id = project.project_id.clone();
    let administers = telmoni_shared::rbac::can(project.role, Verb::Delete, Resource::Trace);
    let projects = Projects::one(project_id.clone());
    let given: Option<ScoreRow> = state
        .store
        .read(
            &projects,
            "SELECT * FROM telemetry.scores FINAL
              WHERE project_id IN {projects:Array(String)} AND score_id = {score:UUID}
                AND is_deleted = 0 AND source = 'member'",
        )
        .param("score", score_id)
        .fetch_optional()
        .await
        .map_err(read_failed)?;
    let Some(given) = given else {
        return Err(
            AuthError::NotFound("no member's score with that id in this project".into()).into(),
        );
    };
    if given.scorer != acting.user_id.as_str() && !administers {
        return Err(telmoni_shared::AuthzError::Forbidden(
            "a member's score is theirs, or an owner's or admin's, to take back".into(),
        )
        .into());
    }
    let taken = ScoreRow {
        received_at: Utc::now(),
        is_deleted: 1,
        ..given
    };
    chain::write(&state.db, &acting, &project_id, "a score", async |tx| {
        chain::held(
            state.auth.as_ref(),
            &acting,
            tx,
            Verb::Update,
            Resource::Trace,
        )
        .await?;
        chain::file(
            tx,
            &acting,
            &project_id,
            Change {
                action: AuditAction::Updated,
                kind: TelmoniResourceKind::Trace,
                id: if taken.run_id.is_nil() {
                    taken.score_id
                } else {
                    taken.run_id
                },
                metadata: json!({ "score": {
                    "name": taken.name, "target": taken.target, "taken_back": true,
                } }),
            },
        )
        .await?;
        state
            .store
            .insert_scores_once(std::slice::from_ref(&taken), &Uuid::new_v4().to_string())
            .await
            .map_err(|error| {
                crate::store::failed(
                    &error,
                    "a score",
                    "the span store did not take the change; retry",
                )
            })
    })
    .await?;
    tracing::info!(project_id = %project_id, score_id = %score_id, "telemetry: a member's score taken back");
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `GET /internal/v1/telemetry/scores?run_id=…[&session_id=…]` — a run's
/// scores, its spans' and, where named, its session's, newest first.
pub async fn get_scores(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(asked): Query<ScoresAsked>,
) -> Result<Json<Scores>, TelmoniError> {
    let reader = crate::handler::traces::reader(&state, &headers).await?;
    let projects = Projects::one(reader.project_id.clone());
    let rows: Vec<ScoreRow> = state
        .store
        .read(&projects, RUN_SQL)
        .param("run", asked.run_id)
        .param("session", asked.session_id.unwrap_or_default())
        .fetch_all()
        .await
        .map_err(read_failed)?;
    Ok(Json(Scores {
        scores: rows.into_iter().map(Score::from).collect(),
    }))
}
