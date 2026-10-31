//! `POST /otlp/v1/traces` — ingest, OTLP/HTTP, behind a project API key
//! holding the ingest scope ([`telmoni_shared::middleware::api_key`]). The
//! order is the contract's: the rate limit, and then the organization's
//! grants ([`telmoni_shared::seam::admit_new`]), before the body is kept, a
//! refusal reading it out first; the body, no further than its cap; its
//! digest claimed, so a retry with the same bytes is answered rather than
//! written again; then the spans read by the contract, their content dropped
//! or kept as the project's mode says, and their tags held to the cardinality
//! cap ([`cardinality`]), and the rows
//! written once ClickHouse holds them, the content of a project that keeps it
//! sealed in Postgres after them ([`content`]), marked deleted again if their project
//! was deleted meanwhile; and the organization's open consoles told of them
//! ([`crate::live`]).
//! Every answer is OTLP's, in the request's encoding: the response on `200`,
//! and on a refusal a `Status` whose message opens with the problem type,
//! since OTLP asks that shape where every other lane answers a problem
//! document.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes, HttpBody as _, to_bytes};
use axum::extract::{Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Version, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, SubsecRound as _, Utc};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceResponse;
use prost::Message as _;
use serde_json::json;

use telmoni_shared::acting::ActingKey;
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::middleware::problem;
use telmoni_shared::{
    ContentMode, OrInternal, ProblemDetails, ProjectId, TelemetryError, TelmoniError,
};

use crate::contract::{self, BATCH_MAX_BYTES, BATCH_SPANS_MAX};
use crate::ingest::batch::{self, BatchError, SenderClock};
use crate::ingest::cardinality;
use crate::ingest::claims::{self, Claim, Guard};
use crate::ingest::decode::{self, DecodeError, Encoding};
use crate::ingest::report::Report;
use crate::live;
use crate::store::ASYNC_INSERT_WAIT;
use crate::{AppState, content, db, erasure, hashing};

/// The header every `200` carries: the project's content mode, so an SDK
/// learns it from its first export.
pub const CONTENT_MODE_HEADER: HeaderName = HeaderName::from_static("telmoni-content-mode");

/// The header the SDKs send their clock in ([`contract::SENT_AT`]).
const SENT_AT_HEADER: HeaderName = HeaderName::from_static(contract::SENT_AT);

/// The longest an export's body may take to arrive: six times OTel's
/// ten-second export timeout, and far inside the fifteen minutes an
/// organization's deletion waits before its purge.
pub(crate) const BODY_WAIT: Duration = Duration::from_secs(60);

/// The most exports one process holds past their read, every project's
/// together: a slow ClickHouse keeps each until its insert ends, and this,
/// not the pod's memory limit, is where that queue stops.
pub(crate) const IN_FLIGHT_MAX: usize = 64;

/// `POST /otlp/v1/traces`.
pub async fn export(State(state): State<Arc<AppState>>, request: Request) -> Response {
    // Before anything waits: the sender's clock is read against it.
    let arrived = Utc::now();
    let (parts, body) = request.into_parts();
    let Some(key) = parts.extensions.get::<ActingKey>().cloned() else {
        return refuse(
            TelmoniError::Internal("ingest: the request carries no key".into()),
            &parts.headers,
        );
    };
    match handle(&state, &key, &parts, body, arrived).await {
        Ok(response) => response,
        Err(error) => refuse(error, &parts.headers),
    }
}

async fn handle(
    state: &Arc<AppState>,
    key: &ActingKey,
    request: &Parts,
    body: Body,
    arrived: DateTime<Utc>,
) -> Result<Response, TelmoniError> {
    let headers = &request.headers;
    let (encoding, gzipped) = match admit(state, key, headers).await {
        Ok(admitted) => admitted,
        Err(refused) => {
            read_out(request, body, state.body_wait).await;
            return Err(refused);
        }
    };

    // One byte past the cap is read, so that past it is known.
    let Ok(read) = tokio::time::timeout(state.body_wait, to_bytes(body, BATCH_MAX_BYTES + 1)).await
    else {
        return Err(TelemetryError::InvalidBatch(format!(
            "the body did not arrive within {} seconds",
            state.body_wait.as_secs_f64()
        ))
        .into());
    };
    let raw = read.map_err(unread)?;
    // Taken once the body is in, so a sender trickling one holds no slot;
    // refused as a busy store is, which exporters retry. The slot goes with
    // the write, which outlives a hang-up.
    let Ok(slot) = Arc::clone(&state.ingest_slots).try_acquire_owned() else {
        return Err(TelemetryError::StoreUnavailable(
            "ingest is at its in-flight limit; retry in a second".into(),
        )
        .into());
    };
    let bytes = decode::inflate(raw, gzipped).map_err(decode_error)?;

    let digest = claims::digest(&bytes);
    let guard = match claims::claim(&state.redis, &key.project_id, &digest).await {
        Claim::Fresh(guard) => guard,
        Claim::Pending => {
            return Err(TelemetryError::StoreUnavailable(
                "a copy of this export is being written; retry in a second".into(),
            )
            .into());
        }
        Claim::Answered(content_mode) => {
            tracing::debug!(
                project_id = %key.project_id,
                "ingest: an export sent again, answered as it was"
            );
            return answer_again(state, encoding, &key.project_id, &bytes, content_mode).await;
        }
    };

    // Its own task, so an exporter that hangs up cannot cancel the write
    // between its insert and its settling.
    let sender = sender_clock(headers, arrived);
    let token = format!("{}:{digest}", key.project_id.as_str());
    let written = tokio::spawn({
        let state = Arc::clone(state);
        let key = key.clone();
        async move {
            let _slot = slot;
            match write(&state, &key, encoding, &bytes, sender, &token).await {
                Ok((content_mode, body)) => {
                    guard.answer(&state.redis, content_mode).await;
                    Ok((content_mode, body))
                }
                // Whatever became of its rows, ClickHouse writes them once
                // under the token, so the body may be sent again at once.
                Err(error) => {
                    release(guard, &state).await;
                    Err(error)
                }
            }
        }
    });
    match written.await {
        Ok(Ok((content_mode, body))) => Ok(answer(encoding, content_mode, body)),
        Ok(Err(error)) => Err(error),
        Err(died) => Err(TelmoniError::internal("ingest: the write died", died)),
    }
}

/// What is settled before the body is kept: the encodings its headers name,
/// the rate limit and the organization's grants; the encodings, if it may come.
async fn admit(
    state: &AppState,
    key: &ActingKey,
    headers: &HeaderMap,
) -> Result<(Encoding, bool), TelmoniError> {
    let encoding = header(headers, &header::CONTENT_TYPE)
        .and_then(Encoding::of_content_type)
        .ok_or_else(|| {
            TelemetryError::InvalidBatch(
                "Content-Type must be application/json or application/x-protobuf".into(),
            )
        })?;
    let gzipped = match header(headers, &header::CONTENT_ENCODING) {
        None => false,
        Some(value) if value.eq_ignore_ascii_case("identity") => false,
        Some(value) if value.eq_ignore_ascii_case("gzip") => true,
        Some(_) => {
            return Err(TelemetryError::InvalidBatch(
                "Content-Encoding must be gzip, or absent".into(),
            )
            .into());
        }
    };
    // Before the body is kept, so a flood costs its connections and what it
    // sends, read and let go, and no more memory than hyper's buffer for each.
    state
        .rate_limit
        .admit(&state.redis, key.project_id.as_str())
        .await?;
    // A paused organization keeps what it holds and takes in nothing new: a
    // 402, which an exporter drops rather than retries.
    telmoni_shared::seam::admit_new(state.entitlements.as_ref(), &key.organization_id).await?;
    Ok((encoding, gzipped))
}

/// The claimed body, written: the spans read by the contract, the rows put
/// in ClickHouse under `token` and settled ([`settle_written`]), and the
/// console told of them; the answer's mode and body on success.
async fn write(
    state: &Arc<AppState>,
    key: &ActingKey,
    encoding: Encoding,
    bytes: &[u8],
    sender: Option<SenderClock>,
    token: &str,
) -> Result<(ContentMode, Vec<u8>), TelmoniError> {
    let request = decode::request(bytes, encoding).map_err(decode_error)?;
    // Made and sealed on the project's first export, which also files its
    // settings row, so the purges find every project with spans.
    let storage = hashing::storage_for(state, key).await?;
    let mut tx = project_scope(&state.db, &key.project_id).await?;
    let content_mode = db::content_mode(&mut tx, &key.project_id).await?;
    tx.commit().await?;
    let received_at = Utc::now().trunc_subsecs(6);
    let mut batch = batch::read(
        &request,
        &key.project_id,
        received_at,
        sender,
        &|raw| storage.store(raw),
        content_mode == ContentMode::On,
    )
    .map_err(batch_error)?;
    cardinality::hold(
        &state.redis,
        &key.project_id,
        received_at,
        &mut batch.rows,
        &mut batch.report.dropped,
    )
    .await;

    // A run deleted as it still runs sends on: what it sends is not kept.
    let runs = erasure::drop_deleted(state, &key.project_id, &mut batch).await;
    if runs > 0 {
        tracing::debug!(project_id = %key.project_id, runs, "ingest: spans of deleted runs dropped");
    }
    if let Err(error) = state.store.insert_spans_once(&batch.rows, token).await {
        // ClickHouse may finish an insert the client stopped waiting for, as
        // late as its own wait: settled once that has passed.
        if matches!(error, clickhouse::error::Error::TimedOut) && !batch.rows.is_empty() {
            settle_later(state, &key.project_id, ASYNC_INSERT_WAIT);
        }
        return Err(store_failed(error));
    }
    erasure::mark_late(state, &key.project_id, &mut batch).await;
    // After the spans, so no content outlives or precedes its span: a write
    // that fails lets the claim go, and the retry's spans land once under the
    // token while its content is kept, each field once. A purge that took
    // the settings row refuses the content, and the spans are settled all
    // the same.
    let kept = content::keep(
        state,
        &key.project_id,
        received_at,
        std::mem::take(&mut batch.content),
    )
    .await;
    let here = !batch.rows.is_empty() && settle_written(state, &key.project_id).await;
    kept?;

    tracing::debug!(
        project_id = %key.project_id,
        token_id = %key.token_id,
        spans = batch.rows.len(),
        refused = batch.report.refused.len(),
        dropped = batch.report.dropped.len(),
        "ingest: export kept"
    );
    // The organization's open consoles are told, so a page reading this
    // project's spans draws itself again — unless nothing was kept or the
    // project is gone. The answer waits for the check, not the telling.
    if here {
        live::spans_ingested(
            Arc::clone(state),
            key.organization_id.as_str(),
            key.project_id.as_str(),
            batch.rows.iter().map(|row| row.run_id),
        );
    }
    let body = response_body(encoding, &batch.report, content_mode)?;
    Ok((content_mode, body))
}

/// The answer to an export sent again: its own bytes, the first copy's by
/// their digest, read by the contract again under the mode the first answer
/// named, its tags held to the cap again, which keeps the values its first
/// copy did. Nothing is kept, so no user id is hashed and no content sealed.
async fn answer_again(
    state: &AppState,
    encoding: Encoding,
    project_id: &ProjectId,
    bytes: &[u8],
    content_mode: ContentMode,
) -> Result<Response, TelmoniError> {
    let request = decode::request(bytes, encoding).map_err(decode_error)?;
    let now = Utc::now();
    let mut batch = batch::read(
        &request,
        project_id,
        now,
        None,
        &|_| String::new(),
        content_mode == ContentMode::On,
    )
    .map_err(batch_error)?;
    cardinality::hold(
        &state.redis,
        project_id,
        now,
        &mut batch.rows,
        &mut batch.report.dropped,
    )
    .await;
    let body = response_body(encoding, &batch.report, content_mode)?;
    Ok(answer(encoding, content_mode, body))
}

/// OTLP's response to what an export lost, in the request's encoding.
fn response_body(
    encoding: Encoding,
    report: &Report,
    content_mode: ContentMode,
) -> Result<Vec<u8>, TelmoniError> {
    let response = ExportTraceServiceResponse {
        partial_success: report.partial_success(content_mode),
    };
    match encoding {
        Encoding::Json => {
            serde_json::to_vec(&response).or_internal("ingest: the answer did not encode")
        }
        Encoding::Protobuf => Ok(response.encode_to_vec()),
    }
}

/// How long an export waits for auth to say whether its project still exists.
const SETTLE_BUDGET: Duration = Duration::from_secs(1);

/// The waits between a settle's later asks, doubling: about four minutes in
/// all, past an insert ClickHouse finishes on its own.
const SETTLE_LATER: [Duration; 8] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
    Duration::from_secs(64),
    Duration::from_secs(128),
];

/// Whether the export's project still exists. An export racing its
/// project's purge can land after the purge's mark, so a project that is
/// gone has its spans marked deleted again. Auth is asked rather than the
/// settings row, since auth commits the delete before the purge marks and
/// the row goes only after. Without an answer in time, or a mark ClickHouse
/// did not take, it is asked again later ([`settle_later`]), and the rows
/// stand meanwhile.
async fn settle_written(state: &Arc<AppState>, project_id: &ProjectId) -> bool {
    match outlived(state, project_id).await {
        Some(true) => true,
        Some(false) => {
            if !marked_again(state, project_id).await {
                settle_later(state, project_id, Duration::ZERO);
            }
            false
        }
        None => {
            settle_later(state, project_id, Duration::ZERO);
            true
        }
    }
}

/// [`settle_written`] apart from the export's answer, after `first`: auth
/// asked again, and the spans marked deleted again once it says the project
/// is gone, until the waits run out. One at once a project, and one after an
/// insert's wait, since a mark takes every row the project holds, so neither
/// an outage of auth's nor a slow ClickHouse is asked again by each export.
/// Held in memory, so a replica stopped meanwhile leaves the spans until the
/// retention line.
fn settle_later(state: &Arc<AppState>, project_id: &ProjectId, first: Duration) {
    let place = (project_id.clone(), !first.is_zero());
    if !state
        .settling
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(place.clone())
    {
        return;
    }
    let (state, project_id) = (Arc::clone(state), project_id.clone());
    tokio::spawn(async move {
        let _settled = Settling(Arc::clone(&state), place);
        tokio::time::sleep(first).await;
        for wait in SETTLE_LATER {
            match outlived(&state, &project_id).await {
                Some(true) => return,
                Some(false) if marked_again(&state, &project_id).await => return,
                Some(false) | None => tokio::time::sleep(wait).await,
            }
        }
        tracing::warn!(
            project_id = %project_id,
            "ingest: an export's project could not be settled; its spans stay until the \
             retention line if it was deleted"
        );
    });
}

/// A project's place in [`AppState::settling`], given up however its task
/// ends.
struct Settling(Arc<AppState>, (ProjectId, bool));

impl Drop for Settling {
    fn drop(&mut self) {
        self.0
            .settling
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.1);
    }
}

/// Whether auth still holds the project, or `None` without its answer in
/// time.
async fn outlived(state: &AppState, project_id: &ProjectId) -> Option<bool> {
    match tokio::time::timeout(
        SETTLE_BUDGET,
        state.auth.project_homes(std::slice::from_ref(project_id)),
    )
    .await
    {
        Ok(Ok(homes)) => Some(homes.iter().any(|home| home.project_id == *project_id)),
        Ok(Err(e)) => {
            tracing::warn!(
                project_id = %project_id,
                error = %e,
                "ingest: auth could not say whether an export's project outlived its write; \
                 it is asked again"
            );
            None
        }
        Err(_) => {
            tracing::warn!(
                project_id = %project_id,
                "ingest: auth did not say in time whether an export's project outlived its \
                 write; it is asked again"
            );
            None
        }
    }
}

/// A gone project's spans marked deleted again; whether ClickHouse took it.
async fn marked_again(state: &AppState, project_id: &ProjectId) -> bool {
    match state.store.delete_project(project_id).await {
        Ok(()) => {
            tracing::info!(
                project_id = %project_id,
                "ingest: an export's project was deleted as it landed; its spans are marked \
                 deleted again"
            );
            true
        }
        // The error's text can carry ClickHouse's message: log its kind alone.
        Err(error) => {
            tracing::warn!(
                project_id = %project_id,
                kind = crate::store::Fault::of(&error).as_str(),
                "ingest: an export's project was deleted as it landed, and ClickHouse did not \
                 mark its spans deleted again; it is asked again"
            );
            false
        }
    }
}

async fn release(guard: Guard, state: &AppState) {
    guard.release(&state.redis).await;
}

/// A `200`: OTLP's response, in the request's encoding, under the mode.
fn answer(encoding: Encoding, content_mode: ContentMode, body: Vec<u8>) -> Response {
    let mut response = (StatusCode::OK, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(encoding.content_type()),
    );
    response.headers_mut().insert(
        CONTENT_MODE_HEADER,
        HeaderValue::from_static(content_mode.as_str()),
    );
    response
}

/// A refusal, as this lane answers one: OTLP's `Status` in the request's
/// encoding — JSON where the request named neither — its code the gRPC one
/// the HTTP status maps to, its message opening with the problem type; the
/// problem's `Retry-After`, where it has one, on the answer too. The key
/// gate refuses through here as well, so a bad key reads the same.
pub fn refuse(error: TelmoniError, headers: &HeaderMap) -> Response {
    let problem = error.to_problem_details();
    if problem.status >= 500 {
        tracing::error!(error = %error, status = problem.status, "ingest: refused on our side");
    }
    refusal(&problem, headers)
}

/// A path under `/otlp` no lane serves — `/v1/logs` and `/v1/metrics`, from
/// an exporter given `…/otlp` whole — answered as the lane answers, once the
/// body is read out.
pub async fn not_found(request: Request) -> Response {
    let headers = read_out_all(request).await;
    refusal(
        &problem::not_found_details("no such lane under /otlp; traces go to /otlp/v1/traces"),
        &headers,
    )
}

/// A method the lane does not take: `405` with `Allow`, as the lane answers,
/// once the body is read out.
pub async fn method_not_allowed(request: Request) -> Response {
    let headers = read_out_all(request).await;
    let mut response = refusal(&problem::method_not_allowed_details("POST"), &headers);
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST"));
    response
}

/// [`refuse`]'s answer, for any problem document.
fn refusal(problem: &ProblemDetails, headers: &HeaderMap) -> Response {
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let encoding = header(headers, &header::CONTENT_TYPE)
        .and_then(Encoding::of_content_type)
        .unwrap_or(Encoding::Json);
    let message = match &problem.detail {
        Some(detail) => format!("{}: {detail}", problem.type_uri),
        None => problem.type_uri.clone(),
    };
    let code = rpc_code(status);
    let body = match encoding {
        Encoding::Json => json!({ "code": code, "message": message })
            .to_string()
            .into_bytes(),
        Encoding::Protobuf => RpcStatus { code, message }.encode_to_vec(),
    };
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(encoding.content_type()),
    );
    if let Some(secs) = problem.retry_after()
        && let Ok(value) = HeaderValue::from_str(&secs.to_string())
    {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// [`refuse`] for a request whose body is unread, once it is read out: the
/// key gate's refusals and the door's.
pub async fn refuse_unread(error: TelmoniError, request: Request) -> Response {
    refuse(error, &read_out_all(request).await)
}

/// The request's headers, its body read out first.
async fn read_out_all(request: Request) -> HeaderMap {
    let (parts, body) = request.into_parts();
    read_out(&parts, body, BODY_WAIT).await;
    parts.headers
}

/// Reads out a body a refusal leaves unread, to the cap and within `wait`:
/// hyper closes on one, and an exporter still sending loses the refusal to it.
async fn read_out(request: &Parts, mut body: Body, wait: Duration) {
    if announced_past_cap(&request.headers) || waits_to_continue(request) {
        return;
    }
    let mut left = BATCH_MAX_BYTES + 1;
    let _ = tokio::time::timeout(wait, async {
        while left > 0 {
            let Some(Ok(frame)) =
                std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await
            else {
                return;
            };
            left = left.saturating_sub(frame.data_ref().map_or(0, Bytes::len));
        }
    })
    .await;
}

/// Whether the body says it is past the cap, where the close comes however
/// much of it is read.
fn announced_past_cap(headers: &HeaderMap) -> bool {
    header(headers, &header::CONTENT_LENGTH)
        .and_then(|length| length.parse::<usize>().ok())
        .is_some_and(|length| length > BATCH_MAX_BYTES)
}

/// Whether the client holds its body back for `100 Continue`, as hyper reads
/// one: past HTTP/1.0, by its last `Expect`. Asked for nothing, it sends nothing.
fn waits_to_continue(request: &Parts) -> bool {
    request.version > Version::HTTP_10
        && request
            .headers
            .get_all(header::EXPECT)
            .iter()
            .next_back()
            .is_some_and(|expect| expect.as_bytes().eq_ignore_ascii_case(b"100-continue"))
}

/// `google.rpc.Status`, the one message OTLP borrows that its crate lacks;
/// its `details` left out, which protobuf reads as none.
#[derive(Clone, PartialEq, prost::Message)]
struct RpcStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
}

/// The gRPC code an HTTP status maps to, after grpc-gateway's mappings; OTLP
/// reads no code, its clients acting on the status alone.
const fn rpc_code(status: StatusCode) -> i32 {
    match status {
        StatusCode::BAD_REQUEST => 3,
        StatusCode::UNAUTHORIZED => 16,
        // A plan's refusal: the organization is not in a state to take the
        // export, which asking again will not change.
        StatusCode::PAYMENT_REQUIRED => 9,
        StatusCode::FORBIDDEN => 7,
        StatusCode::NOT_FOUND => 5,
        StatusCode::METHOD_NOT_ALLOWED => 12,
        StatusCode::PAYLOAD_TOO_LARGE | StatusCode::TOO_MANY_REQUESTS => 8,
        StatusCode::SERVICE_UNAVAILABLE => 14,
        _ => 13,
    }
}

/// A failed `to_bytes`: the body's own failure (hyper could not read it)
/// comes boxed as axum's error and is a 400; anything else is the cap's 413.
fn unread(error: axum::Error) -> TelmoniError {
    if error.into_inner().is::<axum::Error>() {
        TelemetryError::InvalidBatch("the body could not be read".into())
    } else {
        TelemetryError::BatchTooLarge {
            max_bytes: BATCH_MAX_BYTES,
        }
    }
    .into()
}

fn decode_error(error: DecodeError) -> TelmoniError {
    match error {
        DecodeError::TooLarge => TelemetryError::BatchTooLarge {
            max_bytes: BATCH_MAX_BYTES,
        },
        DecodeError::TooManySpans(count) => TelemetryError::InvalidBatch(format!(
            "the export holds {count} spans; at most {BATCH_SPANS_MAX}"
        )),
        DecodeError::Invalid(detail) => TelemetryError::InvalidBatch(detail),
    }
    .into()
}

fn batch_error(error: BatchError) -> TelmoniError {
    match error {
        BatchError::NoSpans => TelemetryError::InvalidBatch("the export holds no span".into()),
        BatchError::TooManySpans(count) => TelemetryError::InvalidBatch(format!(
            "the export holds {count} spans; at most {BATCH_SPANS_MAX}"
        )),
    }
    .into()
}

/// ClickHouse did not take the rows, or was not heard to: an outage the
/// exporter retries, a shape of ours it refused, which retrying will not
/// mend, or no answer in time, after which a retry is written once.
fn store_failed(error: clickhouse::error::Error) -> TelmoniError {
    crate::store::failed(
        &error,
        "ingest",
        "the span store did not take the rows; retry",
    )
}

/// The sender's clock as its export left, by its [`contract::SENT_AT`], beside
/// ours as it `arrived`; `None` where that names no time.
fn sender_clock(headers: &HeaderMap, arrived: DateTime<Utc>) -> Option<SenderClock> {
    let nanos = header(headers, &SENT_AT_HEADER)?
        .parse::<i64>()
        .ok()
        .filter(|nanos| *nanos > 0)?;
    Some(SenderClock {
        sent_at: DateTime::from_timestamp_nanos(nanos),
        arrived,
    })
}

fn header<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_is_a_status_in_the_request_encoding_opening_with_the_type() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let refused = refuse(
            TelemetryError::InvalidBatch("the export holds no span".into()).into(),
            &headers,
        );
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            refused.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );

        let refused = refuse(
            TelemetryError::StoreUnavailable("retry".into()).into(),
            &HeaderMap::new(),
        );
        assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(refused.headers().get(header::RETRY_AFTER).unwrap(), "1");
        assert_eq!(
            refused.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json",
            "no Content-Type on the request: JSON"
        );
    }

    #[test]
    fn a_protobuf_refusal_decodes_as_a_status() {
        let encoded = RpcStatus {
            code: 7,
            message: "/errors/authz/forbidden: no".into(),
        }
        .encode_to_vec();
        let decoded = RpcStatus::decode(encoded.as_slice()).unwrap();
        assert_eq!(decoded.code, 7);
        assert!(decoded.message.starts_with("/errors/authz/forbidden"));
    }

    /// Beside the export's arrival; a header naming no time is none.
    #[test]
    fn the_senders_clock_is_read_from_its_header() {
        let arrived = DateTime::from_timestamp(1_760_000_000, 0).unwrap();
        let sent_at = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(SENT_AT_HEADER, HeaderValue::from_str(value).unwrap());
            sender_clock(&headers, arrived).map(|clock| {
                assert_eq!(clock.arrived, arrived);
                clock.sent_at
            })
        };
        assert_eq!(
            sent_at("1760003600000000000"),
            DateTime::from_timestamp(1_760_003_600, 0)
        );
        assert_eq!(
            sent_at(" 1759996400000000001 "),
            DateTime::from_timestamp(1_759_996_400, 1)
        );
        for none in ["", "0", "-5", "soon", "1.76e18", "99999999999999999999"] {
            assert_eq!(sent_at(none), None, "{none:?}");
        }
        assert_eq!(sender_clock(&HeaderMap::new(), arrived), None);
    }

    /// A stray path or method is answered in the lane's shape: a `Status`
    /// naming the problem, in the request's encoding, `Allow` on a `405`.
    #[tokio::test]
    async fn a_stray_path_or_method_is_answered_as_the_lane_answers() {
        let request = |method: &str, content_type: &str| {
            Request::builder()
                .method(method)
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from("{}"))
                .unwrap()
        };
        let body = |response: Response| async {
            to_bytes(response.into_body(), usize::MAX).await.unwrap()
        };

        let missing = not_found(request("POST", "application/json")).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            missing.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            body(missing).await,
            r#"{"code":5,"message":"/errors/not-found: no such lane under /otlp; traces go to /otlp/v1/traces"}"#
        );

        let wrong = method_not_allowed(request("PUT", "application/x-protobuf")).await;
        assert_eq!(wrong.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(wrong.headers().get(header::ALLOW).unwrap(), "POST");
        let status = RpcStatus::decode(body(wrong).await).unwrap();
        assert_eq!(status.code, 12);
        assert_eq!(
            status.message,
            "/errors/method-not-allowed: this lane takes POST"
        );
    }

    #[test]
    fn http_statuses_map_to_grpc_codes() {
        assert_eq!(rpc_code(StatusCode::BAD_REQUEST), 3);
        assert_eq!(rpc_code(StatusCode::UNAUTHORIZED), 16);
        assert_eq!(rpc_code(StatusCode::PAYMENT_REQUIRED), 9);
        assert_eq!(rpc_code(StatusCode::FORBIDDEN), 7);
        assert_eq!(rpc_code(StatusCode::NOT_FOUND), 5);
        assert_eq!(rpc_code(StatusCode::METHOD_NOT_ALLOWED), 12);
        assert_eq!(rpc_code(StatusCode::PAYLOAD_TOO_LARGE), 8);
        assert_eq!(rpc_code(StatusCode::TOO_MANY_REQUESTS), 8);
        assert_eq!(rpc_code(StatusCode::SERVICE_UNAVAILABLE), 14);
        assert_eq!(rpc_code(StatusCode::INTERNAL_SERVER_ERROR), 13);
    }
}
