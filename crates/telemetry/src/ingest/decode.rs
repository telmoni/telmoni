//! An export's body read into the one request type: gzip undone under the
//! batch's cap, then OTLP's JSON or its protobuf, whichever the request's
//! `Content-Type` names. Nothing here quotes the body back: what a span
//! carries is the sender's, and an error names a place, never a value.

use std::borrow::Cow;
use std::io::Read as _;

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message as _;

use crate::contract::BATCH_MAX_BYTES;

/// The two encodings OTLP/HTTP speaks; an answer is in its request's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// `application/json`: OTLP's JSON, which our SDKs send.
    Json,
    /// `application/x-protobuf`, which most other exporters send.
    Protobuf,
}

impl Encoding {
    /// The encoding a `Content-Type` names, its parameters ignored; `None`
    /// for anything but the two.
    #[must_use]
    pub fn of_content_type(value: &str) -> Option<Self> {
        let media = value.split(';').next().unwrap_or_default().trim();
        if media.eq_ignore_ascii_case("application/json") {
            Some(Self::Json)
        } else if media.eq_ignore_ascii_case("application/x-protobuf")
            || media.eq_ignore_ascii_case("application/protobuf")
        {
            Some(Self::Protobuf)
        } else {
            None
        }
    }

    /// The `Content-Type` an answer in this encoding carries.
    #[must_use]
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Json => "application/json",
            Self::Protobuf => "application/x-protobuf",
        }
    }
}

/// Why a body could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Past [`BATCH_MAX_BYTES`], once decompressed.
    TooLarge,
    /// Not gzip, or not the encoding named: what was wrong, in words that
    /// quote nothing of the body.
    Invalid(String),
}

/// The body as sent, or inflated where `Content-Encoding` said gzip —
/// reading only as far as the cap, since a small body may unfold to a very
/// large one.
///
/// # Errors
///
/// The body, inflated, is past the cap; or it is not gzip.
pub fn inflate(body: &[u8], gzipped: bool) -> Result<Cow<'_, [u8]>, DecodeError> {
    if !gzipped {
        return if body.len() > BATCH_MAX_BYTES {
            Err(DecodeError::TooLarge)
        } else {
            Ok(Cow::Borrowed(body))
        };
    }
    let cap = u64::try_from(BATCH_MAX_BYTES).unwrap_or(u64::MAX);
    let mut inflated = Vec::new();
    flate2::read::GzDecoder::new(body)
        .take(cap.saturating_add(1))
        .read_to_end(&mut inflated)
        .map_err(|_| DecodeError::Invalid("the body is not gzip".into()))?;
    if inflated.len() > BATCH_MAX_BYTES {
        return Err(DecodeError::TooLarge);
    }
    Ok(Cow::Owned(inflated))
}

/// The request `bytes` hold, read as `encoding`.
///
/// # Errors
///
/// The bytes are not an `ExportTraceServiceRequest` in that encoding.
pub fn request(bytes: &[u8], encoding: Encoding) -> Result<ExportTraceServiceRequest, DecodeError> {
    match encoding {
        Encoding::Json => serde_json::from_slice(bytes).map_err(|e| {
            DecodeError::Invalid(format!(
                "the body is not OTLP's JSON ExportTraceServiceRequest: {} at line {}, column {}",
                json_fault(&e),
                e.line(),
                e.column()
            ))
        }),
        Encoding::Protobuf => ExportTraceServiceRequest::decode(bytes).map_err(|e| {
            DecodeError::Invalid(format!(
                "the body is not OTLP's protobuf ExportTraceServiceRequest: {e}"
            ))
        }),
    }
}

/// What went wrong with a JSON body, in a word: serde's own message would
/// quote the value it tripped on.
fn json_fault(error: &serde_json::Error) -> &'static str {
    match error.classify() {
        serde_json::error::Category::Io => "unreadable",
        serde_json::error::Category::Syntax => "a syntax error",
        serde_json::error::Category::Data => "a value of the wrong type",
        serde_json::error::Category::Eof => "cut short",
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

    use super::*;

    #[test]
    fn the_content_type_names_the_encoding_and_nothing_else_is_one() {
        assert_eq!(
            Encoding::of_content_type("application/json; charset=utf-8"),
            Some(Encoding::Json)
        );
        assert_eq!(
            Encoding::of_content_type("APPLICATION/X-PROTOBUF"),
            Some(Encoding::Protobuf)
        );
        assert_eq!(
            Encoding::of_content_type("application/protobuf"),
            Some(Encoding::Protobuf)
        );
        assert_eq!(Encoding::of_content_type("text/plain"), None);
        assert_eq!(Encoding::of_content_type(""), None);
    }

    /// OTLP's JSON spells ids in hex and 64-bit numbers as strings, though
    /// some exporters send the numbers as numbers: both are read.
    #[test]
    fn json_reads_hex_ids_and_nanos_as_strings_or_numbers() {
        let body = br#"{"resourceSpans":[{"scopeSpans":[{"scope":{"name":"telmoni"},"spans":[
            {"traceId":"4bf92f3577b34da6a3ce929d0e0e4736","spanId":"00f067aa0ba902b7",
             "name":"chat","startTimeUnixNano":"1760000000000000000",
             "endTimeUnixNano":1760000001500000000,"kind":3,
             "attributes":[{"key":"gen_ai.usage.input_tokens","value":{"intValue":"1200"}}]}
        ]}]}]}"#;
        let request = request(body, Encoding::Json).expect("OTLP's JSON");
        let span = &request.resource_spans[0].scope_spans[0].spans[0];
        assert_eq!(
            span.trace_id,
            hex_literal(b"4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(span.span_id, hex_literal(b"00f067aa0ba902b7"));
        assert_eq!(span.start_time_unix_nano, 1_760_000_000_000_000_000);
        assert_eq!(span.end_time_unix_nano, 1_760_000_001_500_000_000);
        let tokens = span.attributes[0]
            .value
            .as_ref()
            .and_then(|v| v.value.clone());
        assert!(
            matches!(
                tokens,
                Some(opentelemetry_proto::tonic::common::v1::any_value::Value::IntValue(1_200))
            ),
            "an int sent as a string reads as the int"
        );
    }

    #[test]
    fn protobuf_round_trips() {
        let sent = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans: vec![Span {
                        name: "chat".into(),
                        trace_id: vec![1; 16],
                        span_id: vec![2; 8],
                        ..Span::default()
                    }],
                    ..ScopeSpans::default()
                }],
                ..ResourceSpans::default()
            }],
        };
        let bytes = sent.encode_to_vec();
        assert_eq!(request(&bytes, Encoding::Protobuf).expect("protobuf"), sent);
        let refused = request(b"\xff\xff\xff", Encoding::Protobuf).unwrap_err();
        assert!(matches!(refused, DecodeError::Invalid(_)), "{refused:?}");
    }

    /// ⚠ A refusal names where the body went wrong, never what it said: a
    /// prompt sent where a list was expected comes back as its line.
    #[test]
    fn an_invalid_json_body_is_refused_without_being_quoted() {
        let body = br#"{"resourceSpans": "THE-WHOLE-PROMPT"}"#;
        let DecodeError::Invalid(detail) = request(body, Encoding::Json).unwrap_err() else {
            panic!("a wrong type is invalid, not too large");
        };
        assert!(!detail.contains("THE-WHOLE-PROMPT"), "{detail}");
        assert!(
            detail.contains("a value of the wrong type at line 1"),
            "{detail}"
        );
    }

    #[test]
    fn gzip_is_undone_under_the_cap() {
        let plain = br#"{"resourceSpans":[]}"#;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(plain).expect("compress");
        let gzipped = encoder.finish().expect("compress");
        assert_eq!(inflate(&gzipped, true).expect("gzip").as_ref(), plain);
        assert_eq!(inflate(plain, false).expect("identity").as_ref(), plain);
        assert_eq!(
            inflate(b"not gzip", true).unwrap_err(),
            DecodeError::Invalid("the body is not gzip".into())
        );
    }

    /// A kilobyte of gzip can unfold to a megabyte: the cap holds on what it
    /// unfolds to, read no further than the cap itself.
    #[test]
    fn a_body_past_the_cap_once_inflated_is_too_large() {
        let huge = vec![0u8; BATCH_MAX_BYTES + 1];
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&huge).expect("compress");
        let gzipped = encoder.finish().expect("compress");
        assert!(
            gzipped.len() < 8 * 1024,
            "zeros compress: {}",
            gzipped.len()
        );
        assert_eq!(inflate(&gzipped, true).unwrap_err(), DecodeError::TooLarge);
        assert_eq!(inflate(&huge, false).unwrap_err(), DecodeError::TooLarge);
        let at_cap = vec![0u8; BATCH_MAX_BYTES];
        assert!(inflate(&at_cap, false).is_ok());
    }

    fn hex_literal(hex: &[u8]) -> Vec<u8> {
        hex.chunks(2)
            .map(|pair| {
                let text = std::str::from_utf8(pair).expect("ascii");
                u8::from_str_radix(text, 16).expect("hex")
            })
            .collect()
    }
}
