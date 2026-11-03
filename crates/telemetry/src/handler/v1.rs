//! Telemetry's `/v1` lanes: behind a project API key holding the `ingest`
//! scope, the files an SDK lifts out of a span's messages before it exports
//! the span, each kept once by the hash of its bytes ([`router`]); behind one
//! holding `read`, the project's traces, its rollups and its incidents, as
//! the console reads them ([`reads`]), which auth's `/v1` router puts behind
//! its own gate, the organization's switch and the day's data out.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::middleware;
use axum::response::Response;
use axum::routing::get;
use axum::routing::{delete, post, put};
use serde::Deserialize;
use uuid::Uuid;

use telmoni_shared::TelmoniError;
use telmoni_shared::acting::ActingKey;
use telmoni_shared::digest::sha256_hex;
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::middleware::api_key::{ApiKeyGate, refuse_as_problem, require_api_key};
use telmoni_shared::middleware::problem;
use telmoni_shared::openapi::{Answer, Method, NO_READ_SCOPE, Param, Route};
use telmoni_shared::types::ApiKeyScope;

use crate::AppState;
use crate::content::{self, MEDIA_MAX};
use crate::handler::incidents::{self, IncidentList, IncidentQuery};
use crate::handler::traces::{self, Reader, TraceDetail, TraceList};
use crate::handler::{evaluators, scores};
use crate::rollups::{Metrics, MetricsAsked};
use crate::rules::incidents::IncidentRow;

/// One `/v1` lane this module serves: what it is, and which handler answers.
pub struct V1Lane {
    /// Which handler answers.
    pub op: V1Op,
    /// The scope its key must hold.
    pub scope: ApiKeyScope,
    /// What a customer is told about it.
    pub route: Route,
}

/// Every `/v1` handler this module has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V1Op {
    /// [`put_media`]
    Media,
    /// [`scores::post_scores`]
    Scores,
    /// [`list_traces`]
    Traces,
    /// [`get_trace`]
    Trace,
    /// [`get_metrics`]
    Metrics,
    /// [`list_incidents`]
    Incidents,
    /// [`get_incident`]
    Incident,
    /// [`list_evaluators`]
    Evaluators,
    /// [`get_evaluator`]
    Evaluator,
    /// [`post_evaluator`]
    EvaluatorNew,
    /// [`post_evaluator_version`]
    EvaluatorVersion,
    /// [`delete_evaluator`]
    EvaluatorGone,
}

/// A change's refusal for a key that holds `read` without `evaluate`.
const NO_EVALUATE_SCOPE: Answer = Answer {
    status: 403,
    description: "A key without both the `read` and the `evaluate` scope.",
};

/// What an evaluator's change takes, as the console's.
const EVALUATOR_BODY: &str = "The body is an evaluator as the console saves one: \
    `name`, `kind` (`prompt`, unless named, or `typed`) and, for a prompt, \
    `messages` (system, user and assistant, each a `role` and `content` \
    whose `{{variables}}` a rule fills), `score` (its `name`, `data_type` and, \
    for a number, its range, for a category its options, each described for \
    the model), `reasoning` and `media`, whether it sends the files its \
    variables name; for a typed one, on a System One connection, \
    its `state` (what is judged, its `{{variables}}` filled the same way) and \
    1 to 16 `questions`, each a `name`, `instructions` and `type`: `noul` with \
    optional `yes_means` and `no_means`, `score` with 2 to 10 `levels`, lowest \
    first, or `choice` with 2 to 255 `options`, each a `name` and optionally its `means`. \
    Optionally `connection_id`, `model`, `temperature` (0 to 2, a prompt's \
    alone) and `weekly_limit` in US dollars. Its model is asked once for an \
    answer in its form first, and one that cannot give it saves nothing. \
    Refused on a project that keeps no content.";

/// Every lane this module serves under `/v1`.
pub const V1_LANES: &[V1Lane] = &[
    V1Lane {
        op: V1Op::Media,
        scope: ApiKeyScope::Ingest,
        route: Route {
            method: Method::Put,
            bearer: true,
            path: "/v1/media/{sha256}",
            summary: "Keep a file a message carried",
            description: "An image, audio or a file a span's message carried, sent \
                      once by the SHA-256 of its bytes, which its message then \
                      names as `telmoni:` and the hash. Kept, sealed, for a \
                      project that keeps content, and nothing for one that \
                      does not; sent again, it changes nothing.",
            params: &[Param {
                name: "sha256",
                in_query: false,
                required: true,
                description: "The SHA-256 of the body, in lowercase hex.",
            }],
            answers: &[
                Answer {
                    status: 204,
                    description: "Kept, kept already, or nothing kept for a project that keeps no content or an organization that holds the content its plan does.",
                },
                Answer {
                    status: 400,
                    description: "A hash that is not the body's.",
                },
                Answer {
                    status: 413,
                    description: "A body past 5 MiB.",
                },
            ],
        },
    },
    V1Lane {
        op: V1Op::Scores,
        scope: ApiKeyScope::Ingest,
        route: Route {
            method: Method::Post,
            bearer: true,
            path: "/v1/scores",
            summary: "Score runs, spans and sessions",
            description: "Up to 100 scores, as `{\"scores\": [...]}`, of runs, \
                          spans or sessions that already ended; a score on a span \
                          still open rides on it as OpenTelemetry's \
                          `gen_ai.evaluation.result` event instead. Each names a \
                          `run_id`, a `run_id` and a `span_id`, or a `session_id`; a \
                          `name` of 1 to 64 characters; a `data_type` of \
                          `number`, with a finite `value` and an optional \
                          `range_min` and `range_max`, `boolean`, with a `value` of \
                          1 or 0, or `category`, with 1 to 10 `categories`; and \
                          optionally a `version` of what scored, up to 64 \
                          characters, and an `id`, a UUID of the caller's, under \
                          which a score sent again replaces itself. What it scores \
                          is not looked up, since a run's spans may land after its \
                          score. A score is metadata, kept in every content mode, \
                          and a request is metered as an export is.",
            params: &[],
            answers: &[
                Answer {
                    status: 204,
                    description: "Every score kept.",
                },
                Answer {
                    status: 400,
                    description: "No score, more than 100, a body past 512 KiB, or one breaking a score's rules, named by its place; none is kept.",
                },
                Answer {
                    status: 402,
                    description: "The organization is paused, and takes in nothing new.",
                },
            ],
        },
    },
    V1Lane {
        op: V1Op::Traces,
        scope: ApiKeyScope::Read,
        route: Route {
            method: Method::Get,
            bearer: true,
            path: "/v1/traces",
            summary: "The project's traces",
            description: "A page of the project's traces in the window, \
                          filtered and sorted by the same query the console's \
                          Traces page sends, with how many the filter admits: \
                          each filter is a parameter of its own, repeated for \
                          more than one value, as the API reference lists. The \
                          window reaches back no further than the plan keeps \
                          spans.",
            params: &[
                Param {
                    name: "days",
                    in_query: true,
                    required: false,
                    description: "How many days back: 1 to 360, 7 when left out.",
                },
                Param {
                    name: "sort",
                    in_query: true,
                    required: false,
                    description: "The column to sort by, as the Traces page sorts.",
                },
                Param {
                    name: "dir",
                    in_query: true,
                    required: false,
                    description: "`asc` or `desc`; the column's own when left out.",
                },
                Param {
                    name: "limit",
                    in_query: true,
                    required: false,
                    description: "The most traces a page holds.",
                },
                Param {
                    name: "page",
                    in_query: true,
                    required: false,
                    description: "Which page, from 1.",
                },
            ],
            answers: &[
                Answer {
                    status: 200,
                    description: "A page of traces.",
                },
                Answer {
                    status: 400,
                    description: "A filter, a sort or a page that does not parse, or a search, which is the console's alone.",
                },
                NO_READ_SCOPE,
            ],
        },
    },
    V1Lane {
        op: V1Op::Trace,
        scope: ApiKeyScope::Read,
        route: Route {
            method: Method::Get,
            bearer: true,
            path: "/v1/traces/{run_id}",
            summary: "One trace's spans",
            description: "One trace's spans, the run's own among them, in the \
                          order they started, and what the run spent. With \
                          `content=true`, each span's content too, where the \
                          project keeps it: every such read is filed in the \
                          organization's audit log as the key's.",
            params: &[
                Param {
                    name: "run_id",
                    in_query: false,
                    required: true,
                    description: "The run's id, a UUID.",
                },
                Param {
                    name: "content",
                    in_query: true,
                    required: false,
                    description: "`true` for each span's content where the project keeps it.",
                },
            ],
            answers: &[
                Answer {
                    status: 200,
                    description: "The trace.",
                },
                Answer {
                    status: 400,
                    description: "A run id that is not a UUID.",
                },
                NO_READ_SCOPE,
                Answer {
                    status: 404,
                    description: "No such run in the project, or one older than the plan keeps.",
                },
            ],
        },
    },
    V1Lane {
        op: V1Op::Metrics,
        scope: ApiKeyScope::Read,
        route: Route {
            method: Method::Get,
            bearer: true,
            path: "/v1/metrics",
            summary: "The project's totals by the hour or the day",
            description: "The project's spans, runs, failures, tokens, spend and durations \
                          over the window, by the hour or the day, from the \
                          totals kept an hour at a time, and grouped, if asked, \
                          by one of agent, model, provider, tool, status, \
                          environment or release; or by a tag's values, the \
                          end user or the session, from the spans the plan \
                          keeps, the ten values that spent most each and the \
                          rest summed as one.",
            params: &[
                Param {
                    name: "days",
                    in_query: true,
                    required: false,
                    description: "The window, in days back from now: 1 to 360, 7 when left out, no further than the plan keeps spans.",
                },
                Param {
                    name: "step",
                    in_query: true,
                    required: false,
                    description: "`hour`, or `day`, the default, each in UTC.",
                },
                Param {
                    name: "group",
                    in_query: true,
                    required: false,
                    description: "One of `agent`, `model`, `provider`, `tool`, `status`, `environment`, `release`, `tag`, `user` or `session`.",
                },
                Param {
                    name: "tag",
                    in_query: true,
                    required: false,
                    description: "With `group=tag`, the tag key whose values split the totals.",
                },
            ],
            answers: &[
                Answer {
                    status: 200,
                    description: "The totals, oldest first.",
                },
                Answer {
                    status: 400,
                    description: "A window, a step, a group or a tag key it does not take.",
                },
                NO_READ_SCOPE,
            ],
        },
    },
    V1Lane {
        op: V1Op::Incidents,
        scope: ApiKeyScope::Read,
        route: Route {
            method: Method::Get,
            bearer: true,
            path: "/v1/incidents",
            summary: "The project's incidents",
            description: "The project's incidents, those not resolved first, \
                          then the newest, 200 at most; with `run`, those whose \
                          notices showed that run.",
            params: &[Param {
                name: "run",
                in_query: true,
                required: false,
                description: "A run's id, a UUID.",
            }],
            answers: &[
                Answer {
                    status: 200,
                    description: "The incidents.",
                },
                Answer {
                    status: 400,
                    description: "A run that is not a UUID.",
                },
                NO_READ_SCOPE,
            ],
        },
    },
    V1Lane {
        op: V1Op::Incident,
        scope: ApiKeyScope::Read,
        route: Route {
            method: Method::Get,
            bearer: true,
            path: "/v1/incidents/{id}",
            summary: "One incident",
            description: "One of the project's incidents.",
            params: &[Param {
                name: "id",
                in_query: false,
                required: true,
                description: "The incident's id, a UUID.",
            }],
            answers: &[
                Answer {
                    status: 200,
                    description: "The incident.",
                },
                NO_READ_SCOPE,
                Answer {
                    status: 404,
                    description: "No such incident in the project.",
                },
            ],
        },
    },
    V1Lane {
        op: V1Op::Evaluators,
        scope: ApiKeyScope::Read,
        route: Route {
            method: Method::Get,
            bearer: true,
            path: "/v1/evaluators",
            summary: "The project's evaluators",
            description: "Each of the project's evaluators at its newest version, by \
                          name, its versions counted, and whether the project keeps \
                          the content an evaluator reads.",
            params: &[],
            answers: &[
                Answer {
                    status: 200,
                    description: "The evaluators.",
                },
                NO_READ_SCOPE,
            ],
        },
    },
    V1Lane {
        op: V1Op::Evaluator,
        scope: ApiKeyScope::Read,
        route: Route {
            method: Method::Get,
            bearer: true,
            path: "/v1/evaluators/{id}",
            summary: "One evaluator's versions",
            description: "One evaluator's versions, its newest first.",
            params: &[Param {
                name: "id",
                in_query: false,
                required: true,
                description: "The evaluator's id, a UUID.",
            }],
            answers: &[
                Answer {
                    status: 200,
                    description: "Its versions.",
                },
                NO_READ_SCOPE,
                Answer {
                    status: 404,
                    description: "No evaluator with that id in the project.",
                },
            ],
        },
    },
    V1Lane {
        op: V1Op::EvaluatorNew,
        scope: ApiKeyScope::Evaluate,
        route: Route {
            method: Method::Post,
            bearer: true,
            path: "/v1/evaluators",
            summary: "Make an evaluator",
            description: EVALUATOR_BODY,
            params: &[],
            answers: &[
                Answer {
                    status: 200,
                    description: "The evaluator, at version 1.",
                },
                Answer {
                    status: 400,
                    description: "A change an evaluator's rules refuse, a model that cannot answer in its form, a connection of the other kind's protocol, a project that keeps no content, or one past its evaluators.",
                },
                NO_EVALUATE_SCOPE,
            ],
        },
    },
    V1Lane {
        op: V1Op::EvaluatorVersion,
        scope: ApiKeyScope::Evaluate,
        route: Route {
            method: Method::Post,
            bearer: true,
            path: "/v1/evaluators/{id}/versions",
            summary: "Save an evaluator's next version",
            description: EVALUATOR_BODY,
            params: &[Param {
                name: "id",
                in_query: false,
                required: true,
                description: "The evaluator's id, a UUID.",
            }],
            answers: &[
                Answer {
                    status: 200,
                    description: "The evaluator, at its new version.",
                },
                Answer {
                    status: 400,
                    description: "A change an evaluator's rules refuse, a model that cannot answer in its form, a connection of the other kind's protocol, or a project that keeps no content.",
                },
                NO_EVALUATE_SCOPE,
                Answer {
                    status: 404,
                    description: "No evaluator with that id in the project.",
                },
            ],
        },
    },
    V1Lane {
        op: V1Op::EvaluatorGone,
        scope: ApiKeyScope::Evaluate,
        route: Route {
            method: Method::Delete,
            bearer: true,
            path: "/v1/evaluators/{id}",
            summary: "Delete an evaluator",
            description: "An evaluator and every version of it, its rules judging on \
                          with the evaluators they still name.",
            params: &[Param {
                name: "id",
                in_query: false,
                required: true,
                description: "The evaluator's id, a UUID.",
            }],
            answers: &[
                Answer {
                    status: 204,
                    description: "Deleted.",
                },
                NO_EVALUATE_SCOPE,
                Answer {
                    status: 404,
                    description: "No evaluator with that id in the project.",
                },
            ],
        },
    },
];

/// This module's `/v1` lanes, the `/v1` taken off, behind the ingest key's
/// gate. A method none takes is a problem document outside the gate, as
/// auth's lanes answer one; a path none serves is the merged router's.
pub fn router(state: Arc<AppState>) -> Router {
    let gate = ApiKeyGate {
        auth: state.auth.clone(),
        scope: ApiKeyScope::Ingest,
        refuse: refuse_as_problem,
    };
    lanes(ApiKeyScope::Ingest)
        .layer(DefaultBodyLimit::max(MEDIA_MAX))
        .route_layer(middleware::from_fn_with_state(gate, require_api_key))
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state)
}

/// This module's read lanes, the `/v1` taken off and no gate of their own:
/// auth's `/v1` router puts them behind its key gate, the organization's
/// switch and the day's data out, as it does its own.
pub fn reads(state: Arc<AppState>) -> Router {
    // An evaluator's change takes `read` for the gate, and `evaluate` of its own.
    lanes(ApiKeyScope::Read)
        .merge(lanes(ApiKeyScope::Evaluate))
        .with_state(state)
}

/// Every lane under `scope`, the `/v1` taken off.
fn lanes(scope: ApiKeyScope) -> Router<Arc<AppState>> {
    V1_LANES
        .iter()
        .filter(|lane| lane.scope == scope)
        .fold(Router::new(), |router, lane| {
            let path = lane
                .route
                .path
                .strip_prefix("/v1")
                .unwrap_or(lane.route.path);
            router.route(
                path,
                match lane.op {
                    V1Op::Media => put(put_media),
                    V1Op::Scores => post(scores::post_scores)
                        .layer(DefaultBodyLimit::max(scores::SENT_MAX_BYTES)),
                    V1Op::Traces => get(list_traces),
                    V1Op::Trace => get(get_trace),
                    V1Op::Metrics => get(get_metrics),
                    V1Op::Incidents => get(list_incidents),
                    V1Op::Incident => get(get_incident),
                    V1Op::Evaluators => get(list_evaluators),
                    V1Op::Evaluator => get(get_evaluator),
                    V1Op::EvaluatorNew => post(post_evaluator),
                    V1Op::EvaluatorVersion => post(post_evaluator_version),
                    V1Op::EvaluatorGone => delete(delete_evaluator),
                },
            )
        })
}

/// `GET /v1/traces` — a page of the key's project's traces, as the console's
/// Traces page reads one, but for a search of kept content: answered from its
/// hashes and filed nowhere, a script could probe what the content holds.
pub async fn list_traces(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<TraceList>, TelmoniError> {
    if pairs.iter().any(|(name, _)| name == "q") {
        return Err(TelmoniError::BadRequest(
            "a search of kept content is the console's alone".into(),
        ));
    }
    Ok(Json(
        traces::trace_list(&state, &key.project_id, pairs).await?,
    ))
}

/// What one trace's lane takes.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceAsked {
    #[serde(default)]
    pub content: bool,
}

/// `GET /v1/traces/{run_id}` — one trace of the key's project, its content
/// opened and filed as the key's read where a `GET` asks: a `HEAD` sends no
/// body, so it opens none.
pub async fn get_trace(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
    method: axum::http::Method,
    Path(run_id): Path<String>,
    Query(asked): Query<TraceAsked>,
) -> Result<Json<TraceDetail>, TelmoniError> {
    let reader = Reader::key(&key);
    let with_content = asked.content && method != axum::http::Method::HEAD;
    Ok(Json(
        traces::trace_detail(&state, &reader, &run_id, with_content).await?,
    ))
}

/// `GET /v1/incidents` — the key's project's incidents.
pub async fn list_incidents(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
    Query(query): Query<IncidentQuery>,
) -> Result<Json<IncidentList>, TelmoniError> {
    Ok(Json(
        incidents::incident_list(&state, &key.project_id, query).await?,
    ))
}

/// `GET /v1/incidents/{id}` — one of the key's project's incidents.
pub async fn get_incident(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
    Path(id): Path<String>,
) -> Result<Json<IncidentRow>, TelmoniError> {
    Ok(Json(
        incidents::incident(&state, &key.project_id, &id).await?,
    ))
}

/// `GET /v1/metrics` — the key's project's hourly totals over the window.
pub async fn get_metrics(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
    Query(asked): Query<MetricsAsked>,
) -> Result<Json<Metrics>, TelmoniError> {
    Ok(Json(
        crate::rollups::read(&state, &key.project_id, &asked).await?,
    ))
}

/// `GET /v1/evaluators` — the key's project's evaluators.
pub async fn list_evaluators(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
) -> Result<Json<evaluators::Evaluators>, TelmoniError> {
    Ok(Json(evaluators::listed(&state, &key.project_id).await?))
}

/// `GET /v1/evaluators/{id}` — one of its evaluators' versions.
pub async fn get_evaluator(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
    Path(id): Path<Uuid>,
) -> Result<Json<evaluators::Versions>, TelmoniError> {
    Ok(Json(
        evaluators::versions_of(&state, &key.project_id, id).await?,
    ))
}

/// `POST /v1/evaluators` — a new evaluator, by a key holding `evaluate`.
pub async fn post_evaluator(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
    Json(change): Json<evaluators::EvaluatorChange>,
) -> Result<Json<evaluators::Evaluator>, TelmoniError> {
    key.require_scope(ApiKeyScope::Evaluate)?;
    let change = evaluators::EvaluatorChange { id: None, ..change };
    let project_id = key.project_id.clone();
    Ok(Json(
        evaluators::saved(&state, evaluators::Changer::Key(&key), &project_id, change).await?,
    ))
}

/// `POST /v1/evaluators/{id}/versions` — its next version, by a key holding `evaluate`.
pub async fn post_evaluator_version(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
    Path(id): Path<Uuid>,
    Json(change): Json<evaluators::EvaluatorChange>,
) -> Result<Json<evaluators::Evaluator>, TelmoniError> {
    key.require_scope(ApiKeyScope::Evaluate)?;
    let change = evaluators::EvaluatorChange {
        id: Some(id),
        ..change
    };
    let project_id = key.project_id.clone();
    Ok(Json(
        evaluators::saved(&state, evaluators::Changer::Key(&key), &project_id, change).await?,
    ))
}

/// `DELETE /v1/evaluators/{id}` — an evaluator gone, by a key holding `evaluate`.
pub async fn delete_evaluator(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, TelmoniError> {
    key.require_scope(ApiKeyScope::Evaluate)?;
    let project_id = key.project_id.clone();
    evaluators::removed(&state, evaluators::Changer::Key(&key), &project_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A method an ingest lane does not take: `405` with `Allow`, the lane's own.
async fn method_not_allowed(uri: Uri) -> Response {
    let path = uri.path().strip_prefix("/v1").unwrap_or(uri.path());
    let allow = V1_LANES
        .iter()
        .filter(|lane| lane.scope == ApiKeyScope::Ingest)
        .find(|lane| {
            let fixed = lane
                .route
                .path
                .strip_prefix("/v1")
                .unwrap_or(lane.route.path);
            path.starts_with(fixed.split('{').next().unwrap_or(fixed))
        })
        .map_or(Method::Put, |lane| lane.route.method);
    problem::method_not_allowed(match allow {
        Method::Post => "POST",
        _ => "PUT",
    })
}

/// `PUT /v1/media/{sha256}` — keep a file for the key's project, by the hash
/// of its bytes, which must be the path's.
pub async fn put_media(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
    Path(sha256): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, TelmoniError> {
    // axum refuses a body past the limit in plain text; every `/v1` refusal is a problem.
    let body = body.map_err(|rejection| match rejection.status() {
        StatusCode::PAYLOAD_TOO_LARGE => TelmoniError::PayloadTooLarge {
            max_bytes: u64::try_from(MEDIA_MAX).unwrap_or(u64::MAX),
        },
        _ => TelmoniError::BadRequest("the body could not be read".into()),
    })?;
    if sha256_hex(&body) != sha256 {
        return Err(TelmoniError::BadRequest(
            "the path names a file by the SHA-256 of its body, in lowercase hex".into(),
        ));
    }
    let mime_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|v| is_mime_type(v))
        .unwrap_or("application/octet-stream");
    // Past the organization's content ceiling a file is not kept, as its
    // span's content is not.
    if crate::capacity::has_room_for_content(&state, &key.organization_id).await {
        content::keep_media(&state, &key.project_id, (&sha256, mime_type), &body).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Whether `value` is a media type as a file is drawn by: `type/subtype`, a
/// short run of the characters a token holds, parameters left off.
fn is_mime_type(value: &str) -> bool {
    let token = |s: &str| {
        !s.is_empty()
            && s.len() <= 63
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&b))
    };
    value
        .split_once('/')
        .is_some_and(|(kind, subtype)| token(kind) && token(subtype))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lane's words name what the code keeps: the prefix a message names
    /// a file by, and the cap a 413 refuses past.
    #[test]
    fn the_lanes_words_name_its_prefix_and_its_cap() {
        let route = &V1_LANES[0].route;
        let prefix = format!("`{}`", content::FILE_ID_PREFIX);
        assert!(route.description.contains(&prefix), "{}", route.description);
        let past = route.answers.iter().find(|answer| answer.status == 413);
        let cap = format!("{} MiB", MEDIA_MAX / (1024 * 1024));
        assert!(past.is_some_and(|answer| answer.description.contains(&cap)));
    }

    #[test]
    fn a_media_type_is_type_and_subtype_alone() {
        for good in ["image/png", "audio/mpeg", "application/vnd.ms-excel"] {
            assert!(is_mime_type(good), "{good}");
        }
        for bad in [
            "image",
            "image/",
            "/png",
            "text/html; charset=utf-8",
            "image/p ng",
        ] {
            assert!(!is_mime_type(bad), "{bad}");
        }
    }
}
