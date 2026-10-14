//! `POST /otlp/v1/traces` — ingest, OTLP/HTTP, behind a project API key
//! holding the ingest scope ([`telmoni_shared::middleware::api_key`]). The
//! order is the contract's: the rate limit, and then the organization's
//! grants ([`telmoni_shared::seam::admit_new`]), before the body is read; the
//! body, no further than its cap; its digest claimed, so a retry with the
//! same bytes is answered rather than written again; then the spans read by
//! the contract, their content dropped, and the rows written once ClickHouse
//! holds them, the organization's open consoles told of them
//! ([`crate::ingest::live`]). Every answer is OTLP's, in the request's encoding: the
//! response on `200`, and on a refusal a `Status` whose message opens with
//! the problem type, since OTLP asks that shape where every other lane
//! answers a problem document.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{SubsecRound as _, Utc};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceResponse;
use prost::Message as _;
use serde_json::json;

use telmoni_shared::acting::ActingKey;
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::correlation_id;
use telmoni_shared::{ContentMode, TelemetryError, TelmoniError};

use crate::contract::{BATCH_MAX_BYTES, BATCH_SPANS_MAX};
use crate::ingest::batch::{self, BatchError};
use crate::ingest::claims::{self, Claim, Guard, StoredAnswer};
use crate::ingest::decode::{self, DecodeError, Encoding};
use crate::ingest::live;
use crate::{AppState, db, hashing};

/// The header every `200` carries: the project's content mode, so an SDK
/// learns it from its first export.
pub const CONTENT_MODE_HEADER: HeaderName = HeaderName::from_static("telmoni-content-mode");

/// `POST /otlp/v1/traces`.
pub async fn export(State(state): State<Arc<AppState>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let Some(key) = parts.extensions.get::<ActingKey>().cloned() else {
        return refuse(
            TelmoniError::Internal("ingest: the request carries no key".into()),
            &headers,
        );
    };
    match handle(&state, &key, &headers, body).await {
        Ok(response) => response,
        Err(error) => refuse(error, &headers),
    }
}

async fn handle(
    state: &Arc<AppState>,
    key: &ActingKey,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, TelmoniError> {
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

    // Before the body is read, so a flood costs its connections and no more.
    state
        .rate_limit
        .admit(&state.redis, key.project_id.as_str())
        .await?;
    // A paused organization keeps what it holds and takes in nothing new: a
    // 402, which an exporter drops rather than retries.
    telmoni_shared::seam::admit_new(state.entitlements.as_ref(), &key.organization_id).await?;

    // One byte past the cap is read, so that past it is known; a body that
    // fails to arrive gets no answer anyway.
    let raw =
        to_bytes(body, BATCH_MAX_BYTES + 1)
            .await
            .map_err(|_| TelemetryError::BatchTooLarge {
                max_bytes: BATCH_MAX_BYTES,
            })?;
    let bytes = decode::inflate(&raw, gzipped).map_err(decode_error)?;

    let digest = claims::digest(&bytes);
    let guard = match claims::claim(&state.redis, &key.project_id, &digest).await {
        Claim::Fresh(guard) => guard,
        Claim::Pending => {
            return Err(TelemetryError::StoreUnavailable(
                "a copy of this export is being written; retry in a second".into(),
            )
            .into());
        }
        Claim::Answered(stored) => {
            tracing::debug!(
                project_id = %key.project_id,
                request_id = ?correlation_id(headers),
                "ingest: an export sent again, answered as it was"
            );
            return Ok(answer(encoding, &stored.content_mode, stored.body));
        }
    };

    match write(state, key, headers, encoding, &bytes).await {
        Ok((content_mode, body)) => {
            guard
                .answer(
                    &state.redis,
                    &StoredAnswer {
                        content_mode: content_mode.to_string(),
                        body: body.clone(),
                    },
                )
                .await;
            Ok(answer(encoding, &content_mode.to_string(), body))
        }
        Err(error) => {
            release(guard, state).await;
            Err(error)
        }
    }
}

/// The claimed body, written: the spans read by the contract, the rows put
/// in ClickHouse and the console told of them; the answer's mode and body on
/// success.
async fn write(
    state: &Arc<AppState>,
    key: &ActingKey,
    headers: &HeaderMap,
    encoding: Encoding,
    bytes: &[u8],
) -> Result<(ContentMode, Vec<u8>), TelmoniError> {
    let request = decode::request(bytes, encoding).map_err(decode_error)?;
    // Made and sealed on the project's first export, which also files its
    // settings row, so the purges find every project with spans.
    let storage = hashing::storage_for(state, key).await?;
    let received_at = Utc::now().trunc_subsecs(6);
    let batch = batch::read(&request, &key.project_id, received_at, &|raw| {
        storage.store(raw)
    })
    .map_err(|error| match error {
        BatchError::NoSpans => TelemetryError::InvalidBatch("the export holds no span".into()),
        BatchError::TooManySpans(count) => TelemetryError::InvalidBatch(format!(
            "the export holds {count} spans; at most {BATCH_SPANS_MAX}"
        )),
    })?;

    let mut tx = project_scope(&state.db, &key.project_id).await?;
    let content_mode = db::content_mode(&mut tx, &key.project_id).await?;
    tx.commit().await?;

    state
        .store
        .insert_spans(&batch.rows)
        .await
        .map_err(store_failed)?;

    tracing::debug!(
        project_id = %key.project_id,
        token_id = %key.token_id,
        spans = batch.rows.len(),
        refused = batch.report.refused.len(),
        dropped = batch.report.dropped.len(),
        request_id = ?correlation_id(headers),
        "ingest: export kept"
    );
    // The organization's open consoles are told, so a page reading this
    // project's spans draws itself again, or counts them for the reader — of
    // a batch that kept something: one refused whole stored nothing to read.
    // The answer is not held for it.
    if !batch.rows.is_empty() {
        live::tell(
            Arc::clone(state),
            key.organization_id.as_str(),
            key.project_id.as_str(),
            batch.rows.iter().map(|row| row.run_id),
        );
    }
    let response = ExportTraceServiceResponse {
        partial_success: batch.report.partial_success(content_mode),
    };
    let body = match encoding {
        Encoding::Json => serde_json::to_vec(&response).map_err(|e| {
            TelmoniError::Internal(format!("ingest: the answer did not encode: {e}"))
        })?,
        Encoding::Protobuf => response.encode_to_vec(),
    };
    Ok((content_mode, body))
}

async fn release(guard: Guard, state: &AppState) {
    guard.release(&state.redis).await;
}

/// A `200`: OTLP's response, in the request's encoding, under the mode.
fn answer(encoding: Encoding, content_mode: &str, body: Vec<u8>) -> Response {
    let mut response = (StatusCode::OK, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(encoding.content_type()),
    );
    if let Ok(value) = HeaderValue::from_str(content_mode) {
        response.headers_mut().insert(CONTENT_MODE_HEADER, value);
    }
    response
}

/// A refusal, as this lane answers one: OTLP's `Status` in the request's
/// encoding — JSON where the request named neither — its code the gRPC one
/// the HTTP status maps to, its message opening with the problem type; the
/// problem's `Retry-After`, where it has one, on the answer too. The key
/// gate refuses through here as well, so a bad key reads the same.
pub fn refuse(error: TelmoniError, headers: &HeaderMap) -> Response {
    let problem = error.to_problem_details();
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if status.is_server_error() {
        tracing::error!(error = %error, status = problem.status, "ingest: refused on our side");
    }
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
    if let Some(secs) = problem
        .extensions
        .get("retry_after_secs")
        .and_then(serde_json::Value::as_u64)
        && let Ok(value) = HeaderValue::from_str(&secs.to_string())
    {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
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

/// The gRPC code an HTTP status maps to, as OTLP's own mapping has it.
const fn rpc_code(status: StatusCode) -> i32 {
    match status {
        StatusCode::BAD_REQUEST => 3,
        StatusCode::UNAUTHORIZED => 16,
        // A plan's refusal: the organization is not in a state to take the
        // export, which asking again will not change.
        StatusCode::PAYMENT_REQUIRED => 9,
        StatusCode::FORBIDDEN => 7,
        StatusCode::NOT_FOUND => 5,
        StatusCode::PAYLOAD_TOO_LARGE | StatusCode::TOO_MANY_REQUESTS => 8,
        StatusCode::SERVICE_UNAVAILABLE => 14,
        _ => 13,
    }
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

/// ClickHouse did not take the rows: an outage the exporter retries, or a
/// shape of ours it refused, which retrying will not mend.
fn store_failed(error: clickhouse::error::Error) -> TelmoniError {
    // ⚠ The error's text can carry ClickHouse's own message, which may quote
    // a row: its kind is logged, never the text.
    let kind = crate::store::error_kind(&error);
    if kind == "ours" {
        tracing::error!(kind, "ingest: ClickHouse refused the rows");
        return TelmoniError::Internal("ingest: ClickHouse refused the rows".into());
    }
    tracing::warn!(kind, "ingest: ClickHouse did not take the rows");
    TelemetryError::StoreUnavailable("the span store did not take the rows; retry".into()).into()
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

    #[test]
    fn http_statuses_map_to_grpc_codes() {
        assert_eq!(rpc_code(StatusCode::BAD_REQUEST), 3);
        assert_eq!(rpc_code(StatusCode::UNAUTHORIZED), 16);
        assert_eq!(rpc_code(StatusCode::PAYMENT_REQUIRED), 9);
        assert_eq!(rpc_code(StatusCode::FORBIDDEN), 7);
        assert_eq!(rpc_code(StatusCode::PAYLOAD_TOO_LARGE), 8);
        assert_eq!(rpc_code(StatusCode::TOO_MANY_REQUESTS), 8);
        assert_eq!(rpc_code(StatusCode::SERVICE_UNAVAILABLE), 14);
        assert_eq!(rpc_code(StatusCode::INTERNAL_SERVER_ERROR), 13);
    }
}
