//! A member's scores: given on a trace's page, of the run, one of its spans
//! or its session, filed in the organization's chain; and every score of a
//! run read back, the application's, its members' and its evaluators'.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::Utc;
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::acting::ActingKey;
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, ProjectId, ScoreSource, ScoreTarget, TelmoniError, TelmoniResourceKind,
};

use crate::AppState;
use crate::contract::SESSION_ID_MAX;
use crate::handler::chain::{self, Change};
use crate::handler::traces::read_failed;
use crate::scores::{
    COMMENT_MAX, Judgement, RUN_SQL, ScoreRow, TargetKey, VERSION_MAX, api_score_id,
    member_score_id, version_of,
};
use crate::store::Projects;

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
    /// What the member says of it: content, kept only where the project keeps
    /// content, and opened only with the run's.
    #[serde(default)]
    pub comment: Option<String>,
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
    /// A typed evaluator's answer: its probabilities, confidence and model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<serde_json::Value>,
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
            answer: serde_json::from_str(&row.answer).ok(),
        }
    }
}

/// A run's scores, as `GET /internal/v1/telemetry/scores` answers them.
#[derive(Debug, Serialize)]
pub struct Scores {
    pub scores: Vec<Score>,
}

/// Whether what a score names is the project's: its run, the span in it, or
/// a run of its session; and the run's trace.
const EXISTS_SQL: &str = "SELECT count() AS n, any(trace_id) AS trace FROM telemetry.spans
     WHERE project_id IN {projects:Array(String)}
       AND (({session:String} = '' AND run_id = {run:UUID}
             AND ({span:String} = '' OR span_id = {span:String}))
         OR ({session:String} != '' AND session_id = {session:String}))";

/// Each evaluator's standing score of a target under a name, which a
/// member's of the same is a correction of.
const JUDGED_SQL: &str = "SELECT * FROM telemetry.scores FINAL
     WHERE project_id IN {projects:Array(String)} AND source = 'evaluator' AND is_deleted = 0
       AND name = {name:String} AND target = {target:String} AND run_id = {run:UUID}
       AND span_id = {span:String} AND session_id = {session:String}";

/// The evaluators' scores of `target` under `name`, as corrections are made of them.
async fn judged_of(
    state: &AppState,
    project_id: &ProjectId,
    target: &TargetKey,
    name: &str,
) -> Result<Vec<crate::evaluators::examples::Judged>, TelmoniError> {
    let (run, span, session) = target.ids();
    let rows: Vec<ScoreRow> = state
        .store
        .read(&Projects::one(project_id.clone()), JUDGED_SQL)
        .param("name", name)
        .param("target", target.kind().to_string())
        .param("run", run)
        .param("span", span.as_str())
        .param("session", session.as_str())
        .fetch_all()
        .await
        .map_err(read_failed)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            Some(crate::evaluators::examples::Judged {
                evaluator: Uuid::parse_str(&row.scorer).ok()?,
                judgement: Score::from(row).judgement,
            })
        })
        .collect())
}

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
    trace: String,
}

/// What a score is of, from the ids sent: a run, a run and one of its spans
/// (16 hex digits), or a session.
fn target_of(
    run_id: Option<Uuid>,
    span_id: Option<String>,
    session_id: Option<String>,
) -> Result<TargetKey, TelmoniError> {
    // A session is trimmed, as a span carries one; a span's id is its 16 hex
    // digits as given, so one blank names no run in its place.
    let session = session_id.map(|s| s.trim().to_owned());
    // The nil run is a score's waiting for its trace, never one named.
    match (run_id.filter(|run| !run.is_nil()), span_id, session) {
        (Some(run), None, None) => Ok(TargetKey::Run(run)),
        (Some(run), Some(span), None)
            if span.len() == 16 && span.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            Ok(TargetKey::Span(run, span.to_ascii_lowercase()))
        }
        (None, None, Some(session))
            if run_id.is_none()
                && !session.is_empty()
                && session.chars().count() <= SESSION_ID_MAX =>
        {
            Ok(TargetKey::Session(session))
        }
        _ => Err(TelmoniError::BadRequest(format!(
            "a score names a run, a run and one of its spans by 16 hex digits, or a session \
             of up to {SESSION_ID_MAX} characters"
        ))),
    }
}

/// A comment trimmed, a blank one none; past [`COMMENT_MAX`] characters, refused.
fn comment_of(given: Option<String>) -> Result<Option<String>, TelmoniError> {
    let comment = given.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty());
    if comment
        .as_ref()
        .is_some_and(|c| c.chars().count() > COMMENT_MAX)
    {
        return Err(TelmoniError::BadRequest(format!(
            "a score's comment is up to {COMMENT_MAX} characters"
        )));
    }
    Ok(comment)
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
    let target = target_of(change.run_id, change.span_id, change.session_id)?;
    let comment = comment_of(change.comment)?;
    if comment.is_some() {
        // No run carries a session's comment through an end user's erasure.
        if target.kind() == ScoreTarget::Session {
            return Err(TelmoniError::BadRequest(
                "a session's score takes no comment: comment on one of its runs".into(),
            ));
        }
        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&state.db, &project_id).await?;
        let mode = crate::db::content_mode(&mut tx, &project_id).await?;
        tx.commit().await?;
        if mode != telmoni_shared::ContentMode::On {
            return Err(TelmoniError::BadRequest(
                "this project keeps no content, and a score's comment is content: score it \
                 without one"
                    .into(),
            ));
        }
        if !crate::capacity::has_room_for_content(&state, &acting.organization_id).await {
            return Err(TelmoniError::BadRequest(
                "this organization holds the content its plan does, and a score's comment is \
                 content: score it without one"
                    .into(),
            ));
        }
    }
    let (run, span, session) = target.ids();
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
        target: target.kind(),
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
        answer: String::new(),
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
    // A comment given again replaces the last, and none takes it back.
    if target.kind() != ScoreTarget::Session {
        crate::content::keep_comment(
            &state,
            &project_id,
            (run, &found.trace),
            score_id,
            comment.as_deref(),
        )
        .await?;
    }
    // A correction of an evaluator that scored it so: its example.
    // Best effort, as the example is: the member's score stands.
    let judged = judged_of(&state, &project_id, &target, &row.name)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(project_id = %project_id, error = %error,
                "telemetry: the evaluators a member's score corrects could not be read");
            Vec::new()
        });
    crate::evaluators::examples::learn(
        &state,
        (&project_id, &acting.organization_id),
        &target,
        &judgement,
        &judged,
    )
    .await;
    tracing::info!(project_id = %project_id, score_id = %score_id, target = %row.target, "telemetry: a member scored");
    Ok(Json(Score::from(row)))
}

/// The most scores one `POST /v1/scores` carries, and the most bytes: a
/// hundred at their longest fit well within it.
pub const SENT_MAX: usize = 100;
pub const SENT_MAX_BYTES: usize = 512 * 1024;

/// What `POST /v1/scores` carries: up to [`SENT_MAX`] scores.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoresSent {
    pub scores: Vec<ScoreSent>,
}

/// One score the application sends once what it scores has ended: its own id,
/// so sending it again replaces it, what it scores, and what it says.
#[derive(Debug, Deserialize)]
pub struct ScoreSent {
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub run_id: Option<Uuid>,
    #[serde(default)]
    pub span_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(flatten)]
    pub judgement: Judgement,
}

/// `POST /v1/scores` — the application's scores of runs, spans or sessions
/// of the key's project, sent after they ended, each held to a score's rules
/// and all refused for one that breaks them. Nothing is looked up, since a
/// run's spans may land after its score.
pub async fn post_scores(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
    Json(sent): Json<ScoresSent>,
) -> Result<StatusCode, TelmoniError> {
    // Metered as an export is, and refused a paused organization, which takes
    // in nothing new.
    state
        .rate_limit
        .admit(&state.redis, key.project_id.as_str())
        .await?;
    telmoni_shared::seam::admit_new(state.entitlements.as_ref(), &key.organization_id).await?;
    if sent.scores.is_empty() || sent.scores.len() > SENT_MAX {
        return Err(TelmoniError::BadRequest(format!(
            "a request sends 1 to {SENT_MAX} scores"
        )));
    }
    let now = Utc::now();
    let rows = sent
        .scores
        .into_iter()
        .enumerate()
        .map(|(i, score)| {
            let refused = |error: TelmoniError| match error {
                TelmoniError::BadRequest(why) => {
                    TelmoniError::BadRequest(format!("scores[{i}]: {why}"))
                }
                other => other,
            };
            let judgement = score.judgement.checked().map_err(refused)?;
            let target =
                target_of(score.run_id, score.span_id, score.session_id).map_err(refused)?;
            let version = version_of(score.version.as_deref()).ok_or_else(|| {
                refused(TelmoniError::BadRequest(format!(
                    "a scorer's version is up to {VERSION_MAX} characters"
                )))
            })?;
            let (run_id, span_id, session_id) = target.ids();
            Ok(ScoreRow {
                project_id: key.project_id.clone(),
                received_at: now,
                score_id: score
                    .id
                    .map_or_else(Uuid::new_v4, |id| api_score_id(&key.project_id, id)),
                target: target.kind(),
                run_id,
                trace_id: String::new(),
                span_id,
                session_id,
                value: judgement.stored_value(),
                name: judgement.name,
                data_type: judgement.data_type,
                categories: judgement.categories,
                range_min: judgement.range_min,
                range_max: judgement.range_max,
                source: ScoreSource::Sdk,
                scorer: String::new(),
                scorer_version: version,
                is_deleted: 0,
                answer: String::new(),
            })
        })
        .collect::<Result<Vec<_>, TelmoniError>>()?;
    state
        .store
        .insert_scores_once(&rows, &Uuid::new_v4().to_string())
        .await
        .map_err(|error| {
            crate::store::failed(
                &error,
                "scores",
                "the span store did not take the scores; retry",
            )
        })?;
    tracing::info!(project_id = %key.project_id, token_id = %key.token_id, scores = rows.len(), "telemetry: the application sent scores");
    Ok(StatusCode::NO_CONTENT)
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
    // Its comment first, as the content it was: a retry finds the score
    // still there to take back, where a failure after its mark would leave
    // the comment for good.
    if !given.run_id.is_nil() {
        crate::content::forget_comments(&state, &project_id, given.run_id, &[score_id]).await?;
    }
    // And the corrections it made of the evaluators that scored the same.
    let target = match given.target {
        ScoreTarget::Run => Some(TargetKey::Run(given.run_id)),
        ScoreTarget::Span => Some(TargetKey::Span(given.run_id, given.span_id.clone())),
        ScoreTarget::Session => None,
    };
    if let Some(target) = target {
        let evaluators: Vec<Uuid> = judged_of(&state, &project_id, &target, &given.name)
            .await?
            .into_iter()
            .map(|judged| judged.evaluator)
            .collect();
        crate::evaluators::examples::forget_target(&state, &project_id, &target, &evaluators)
            .await?;
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

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    /// `sdk/fixtures/scores.json`, which every SDK holds its own checks to.
    fn fixture() -> Value {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/fixtures/scores.json");
        serde_json::from_str(&std::fs::read_to_string(path).expect("scores.json"))
            .expect("scores.json is JSON")
    }

    /// A fixture's number, JSON holding no infinity: its non-finite ones are words.
    fn number(value: &Value) -> Option<f64> {
        value.as_f64().or_else(|| value.as_str()?.parse().ok())
    }

    /// The server keeps and refuses each score as the SDKs do.
    #[test]
    fn a_sent_score_is_kept_or_refused_as_the_sdks_fixture_says() {
        for case in fixture()["scores"].as_array().expect("scores") {
            let label = case["case"].as_str().unwrap_or_default();
            let data_type: telmoni_shared::ScoreType =
                serde_json::from_value(case["type"].clone()).expect("a type");
            let judgement = Judgement {
                name: case["name"].as_str().unwrap_or_default().to_owned(),
                data_type,
                value: match data_type {
                    telmoni_shared::ScoreType::Boolean => {
                        Some(if case["value"] == true { 1.0 } else { 0.0 })
                    }
                    telmoni_shared::ScoreType::Number => number(&case["value"]),
                    telmoni_shared::ScoreType::Category => None,
                },
                categories: case["value"]
                    .as_str()
                    .filter(|_| case["type"] == "category")
                    .map(str::to_owned)
                    .into_iter()
                    .collect(),
                range_min: number(&case["range"][0]),
                range_max: number(&case["range"][1]),
            };
            let checked = judgement.checked();
            let version = version_of(case["version"].as_str());
            if case["refused"] == true {
                assert!(checked.is_err() || version.is_none(), "{label}: kept");
                continue;
            }
            let (Ok(judgement), Some(version)) = (checked, version) else {
                panic!("{label}: refused");
            };
            let kept = &case["kept"];
            assert_eq!(
                judgement.name,
                kept["name"].as_str().unwrap_or_default(),
                "{label}"
            );
            assert_eq!(
                version,
                kept["version"].as_str().unwrap_or_default(),
                "{label}"
            );
            match kept["label"].as_str() {
                Some(category) => assert_eq!(judgement.categories, [category], "{label}"),
                None => assert_eq!(judgement.value, kept["value"].as_f64(), "{label}"),
            }
        }
    }

    /// The server takes and refuses each target as the SDKs do.
    #[test]
    fn a_sent_target_is_taken_or_refused_as_the_sdks_fixture_says() {
        for case in fixture()["targets"].as_array().expect("targets") {
            let label = case["case"].as_str().unwrap_or_default();
            let text = |key: &str| case[key].as_str().map(str::to_owned);
            let run = case["run_id"].as_str().map(|run| Uuid::parse_str(run).ok());
            // A run that is no UUID is refused by the body's own parse.
            let target = match run {
                Some(None) => None,
                run => target_of(run.flatten(), text("span_id"), text("session_id")).ok(),
            };
            if case["refused"] == true {
                assert!(target.is_none(), "{label}: taken");
                continue;
            }
            let (run, span, session) = target.unwrap_or_else(|| panic!("{label}: refused")).ids();
            let kept = &case["kept"];
            let or_none = |key: &str| kept[key].as_str().unwrap_or_default().to_owned();
            assert_eq!(
                (run, span, session),
                (
                    kept["run_id"]
                        .as_str()
                        .map_or(Uuid::nil(), |r| Uuid::parse_str(r).expect("a run")),
                    or_none("span_id"),
                    or_none("session_id"),
                ),
                "{label}"
            );
        }
    }

    /// A body sends 1 to [`SENT_MAX`] scores, as `scores.json`'s limits say.
    #[test]
    fn the_fixtures_limits_are_the_servers() {
        let limits = &fixture()["limits"];
        assert_eq!(limits["sent_max"], SENT_MAX);
        assert_eq!(limits["session_max"], SESSION_ID_MAX);
        assert_eq!(limits["name_max"], crate::scores::NAME_MAX);
        assert_eq!(limits["category_max"], crate::scores::CATEGORY_MAX);
        assert_eq!(limits["version_max"], VERSION_MAX);
    }
}
