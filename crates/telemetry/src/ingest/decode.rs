//! An export's body read into the one request type: gzip undone under the
//! batch's cap, then OTLP's JSON or its protobuf, whichever the request's
//! `Content-Type` names. Nothing here quotes the body back: what a span
//! carries is the sender's, and an error names a place, never a value.

use std::io::Read as _;

use axum::body::Bytes;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message as _;
use prost::encoding::{WireType, decode_key, decode_varint};

use crate::contract::{BATCH_MAX_BYTES, BATCH_SPANS_MAX};

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
    /// More spans than [`BATCH_SPANS_MAX`], counted before one was decoded:
    /// how many.
    TooManySpans(usize),
    /// Not gzip, or not the encoding named: what was wrong, in words that
    /// quote nothing of the body.
    Invalid(String),
}

/// The body as sent, or inflated where `Content-Encoding` said gzip, read no
/// further than the cap, since a small body may unfold to a very large one.
///
/// # Errors
///
/// The body, inflated, is past the cap; or it is not gzip.
pub fn inflate(body: Bytes, gzipped: bool) -> Result<Bytes, DecodeError> {
    if !gzipped {
        return if body.len() > BATCH_MAX_BYTES {
            Err(DecodeError::TooLarge)
        } else {
            Ok(body)
        };
    }
    let cap = u64::try_from(BATCH_MAX_BYTES).unwrap_or(u64::MAX);
    let mut inflated = Vec::new();
    flate2::read::GzDecoder::new(body.as_ref())
        .take(cap.saturating_add(1))
        .read_to_end(&mut inflated)
        .map_err(|_| DecodeError::Invalid("the body is not gzip".into()))?;
    if inflated.len() > BATCH_MAX_BYTES {
        return Err(DecodeError::TooLarge);
    }
    Ok(Bytes::from(inflated))
}

/// The request `bytes` hold, read as `encoding`.
///
/// # Errors
///
/// The bytes hold more spans than a batch takes, or are not an
/// `ExportTraceServiceRequest` in that encoding.
pub fn request(bytes: &[u8], encoding: Encoding) -> Result<ExportTraceServiceRequest, DecodeError> {
    let count = span_count(bytes, encoding)?;
    if count > BATCH_SPANS_MAX {
        return Err(DecodeError::TooManySpans(count));
    }
    match encoding {
        Encoding::Json => serde_json::from_slice(bytes).map_err(|e| json_invalid(&e)),
        Encoding::Protobuf => ExportTraceServiceRequest::decode(bytes).map_err(|e| {
            DecodeError::Invalid(format!(
                "the body is not OTLP's protobuf ExportTraceServiceRequest: {e}"
            ))
        }),
    }
}

/// ⚠ **Counted before a span is built, and refused unread where it cannot
/// be counted.** A span of two bytes on the wire is hundreds once decoded,
/// so a body under the byte cap could hold enough empty spans to cost the
/// replica a few hundred megabytes before the span cap refused it — and a
/// decode builds each span before the fault that ends it, so a body this
/// count cannot read goes no further. The count takes all the decode does:
/// every field it does not need, a group among them, is stepped over unread.
fn span_count(bytes: &[u8], encoding: Encoding) -> Result<usize, DecodeError> {
    match encoding {
        Encoding::Json => serde_json::from_slice::<JsonShape>(bytes)
            .map(|shape| {
                shape
                    .resource_spans
                    .iter()
                    .flat_map(|resource| &resource.scope_spans)
                    .map(|scope| scope.spans.len())
                    .fold(0usize, usize::saturating_add)
            })
            .map_err(|e| json_invalid(&e)),
        Encoding::Protobuf => {
            let mut count = 0usize;
            each_field(bytes, 1, |resource_spans| {
                each_field(resource_spans, 2, |scope_spans| {
                    each_field(scope_spans, 2, |_span| {
                        count = count.saturating_add(1);
                        Some(())
                    })
                })
            })
            .ok_or_else(|| {
                DecodeError::Invalid(
                    "the body is not OTLP's protobuf ExportTraceServiceRequest".into(),
                )
            })?;
            Ok(count)
        }
    }
}

/// A JSON body that does not read as the request, in the words every
/// refusal of one uses: where, and what kind of fault, never what it said.
fn json_invalid(error: &serde_json::Error) -> DecodeError {
    DecodeError::Invalid(format!(
        "the body is not OTLP's JSON ExportTraceServiceRequest: {} at line {}, column {}",
        json_fault(error),
        error.line(),
        error.column()
    ))
}

/// OTLP's JSON request as far as its spans, each span read as nothing: the
/// names and defaults `opentelemetry-proto`'s own decoding takes.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct JsonShape {
    resource_spans: Vec<JsonResourceSpans>,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct JsonResourceSpans {
    scope_spans: Vec<JsonScopeSpans>,
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct JsonScopeSpans {
    spans: Vec<serde::de::IgnoredAny>,
}

/// Each length-delimited field numbered `tag` in one protobuf message, in
/// order, every other field stepped over unread: the request's
/// `resource_spans` (1), their `scope_spans` (2) and theirs `spans` (2).
/// `None` where the message does not read as protobuf.
fn each_field(
    mut message: &[u8],
    tag: u32,
    mut on_field: impl FnMut(&[u8]) -> Option<()>,
) -> Option<()> {
    while !message.is_empty() {
        let (number, wire_type) = decode_key(&mut message).ok()?;
        if wire_type == WireType::LengthDelimited {
            let len = usize::try_from(decode_varint(&mut message).ok()?).ok()?;
            let (field, rest) = message.split_at_checked(len)?;
            if number == tag {
                on_field(field)?;
            }
            message = rest;
        } else {
            skip(&mut message, wire_type)?;
        }
    }
    Some(())
}

/// Steps over one field's value of `wire_type`, a group whole: counted by
/// depth rather than by recursion, so groups nested a million deep cost a
/// loop and not the stack.
fn skip(message: &mut &[u8], wire_type: WireType) -> Option<()> {
    let mut depth = 0usize;
    let mut wire_type = wire_type;
    loop {
        match wire_type {
            WireType::Varint => {
                decode_varint(message).ok()?;
            }
            WireType::SixtyFourBit => *message = message.get(8..)?,
            WireType::ThirtyTwoBit => *message = message.get(4..)?,
            WireType::LengthDelimited => {
                let len = usize::try_from(decode_varint(message).ok()?).ok()?;
                *message = message.get(len..)?;
            }
            WireType::StartGroup => depth += 1,
            WireType::EndGroup => depth = depth.checked_sub(1)?,
        }
        if depth == 0 {
            return Some(());
        }
        wire_type = decode_key(message).ok()?.1;
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

    /// The SDKs send a double JSON cannot carry, `NaN` or an infinity, as
    /// `null`: read as no value, alone or in a list, and the export read whole.
    #[test]
    fn a_null_double_reads_as_no_value() {
        use opentelemetry_proto::tonic::common::v1::any_value::Value;
        let body = br#"{"resourceSpans":[{"scopeSpans":[{"spans":[
            {"traceId":"4bf92f3577b34da6a3ce929d0e0e4736","spanId":"00f067aa0ba902b7","name":"chat",
             "attributes":[{"key":"app.nan","value":{"doubleValue":null}},
               {"key":"app.ratios","value":{"arrayValue":{"values":[{"doubleValue":1.5},{"doubleValue":null}]}}}]}
        ]}]}]}"#;
        let request = request(body, Encoding::Json).expect("an export with a null double");
        let attributes = &request.resource_spans[0].scope_spans[0].spans[0].attributes;
        assert_eq!(
            attributes[0].value.as_ref().map(|v| v.value.clone()),
            Some(None)
        );
        let Some(Value::ArrayValue(ratios)) =
            attributes[1].value.as_ref().and_then(|v| v.value.clone())
        else {
            panic!("a list of doubles");
        };
        assert_eq!(ratios.values[0].value, Some(Value::DoubleValue(1.5)));
        assert_eq!(ratios.values[1].value, None);
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
        assert_eq!(
            inflate(Bytes::from(gzipped), true).expect("gzip").as_ref(),
            plain
        );
        let sent = Bytes::from_static(plain);
        let kept = inflate(sent.clone(), false).expect("identity");
        assert_eq!(kept, sent);
        assert_eq!(kept.as_ptr(), sent.as_ptr(), "moved, never copied");
        assert_eq!(
            inflate(Bytes::from_static(b"not gzip"), true).unwrap_err(),
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
        assert_eq!(
            inflate(Bytes::from(gzipped), true).unwrap_err(),
            DecodeError::TooLarge
        );
        assert_eq!(
            inflate(Bytes::from(huge), false).unwrap_err(),
            DecodeError::TooLarge
        );
        let at_cap = vec![0u8; BATCH_MAX_BYTES];
        assert!(inflate(Bytes::from(at_cap), false).is_ok());
    }

    fn hex_literal(hex: &[u8]) -> Vec<u8> {
        hex.chunks(2)
            .map(|pair| {
                let text = std::str::from_utf8(pair).expect("ascii");
                u8::from_str_radix(text, 16).expect("hex")
            })
            .collect()
    }

    fn spans_in_two_resources(first: usize, second: usize) -> ExportTraceServiceRequest {
        let resource = |n: usize| ResourceSpans {
            scope_spans: vec![
                ScopeSpans {
                    spans: vec![Span::default(); n / 2],
                    ..ScopeSpans::default()
                },
                ScopeSpans {
                    spans: vec![Span::default(); n - n / 2],
                    ..ScopeSpans::default()
                },
            ],
            ..ResourceSpans::default()
        };
        ExportTraceServiceRequest {
            resource_spans: vec![resource(first), resource(second)],
        }
    }

    /// The count the cap is held to before anything is decoded is the one
    /// the decode itself would come to, in either encoding.
    #[test]
    fn spans_are_counted_without_decoding_one() {
        let sent = spans_in_two_resources(7, 4);
        let protobuf = sent.encode_to_vec();
        let json = serde_json::to_vec(&sent).expect("json");
        assert_eq!(span_count(&protobuf, Encoding::Protobuf), Ok(11));
        assert_eq!(span_count(&json, Encoding::Json), Ok(11));
        assert!(span_count(b"\xff\xff\xff", Encoding::Protobuf).is_err());
        assert!(span_count(b"[", Encoding::Json).is_err());
    }

    /// ⚠ Empty spans are two bytes each on the wire: past the cap, the body
    /// is refused before one of them is built.
    #[test]
    fn a_body_past_the_span_cap_is_refused_before_it_is_decoded() {
        let sent = spans_in_two_resources(BATCH_SPANS_MAX, 1);
        for (bytes, encoding) in [
            (sent.encode_to_vec(), Encoding::Protobuf),
            (
                br#"{"resourceSpans":[{"scopeSpans":[{"spans":[{}]}]}]}"#.to_vec(),
                Encoding::Json,
            ),
        ] {
            let refused = request(&bytes, encoding).err();
            if encoding == Encoding::Protobuf {
                assert_eq!(
                    refused,
                    Some(DecodeError::TooManySpans(BATCH_SPANS_MAX + 1))
                );
            } else {
                assert!(refused.is_none(), "one span is under the cap");
            }
        }
        let many = format!(
            r#"{{"resourceSpans":[{{"scopeSpans":[{{"spans":[{}]}}]}}]}}"#,
            vec!["{}"; BATCH_SPANS_MAX + 1].join(",")
        );
        assert_eq!(
            request(many.as_bytes(), Encoding::Json).err(),
            Some(DecodeError::TooManySpans(BATCH_SPANS_MAX + 1))
        );
    }

    /// A group, which proto3 never writes and a decoder still steps over,
    /// hides no span after it from the count; one never closed, or nested
    /// deeper than any stack, is refused before a span is built.
    #[test]
    fn a_group_hides_no_span_and_breaks_no_stack() {
        const START: u8 = (15 << 3) | 3;
        const END: u8 = (15 << 3) | 4;
        let mut grouped = vec![START, 8, 1, END];
        grouped.extend_from_slice(&spans_in_two_resources(BATCH_SPANS_MAX, 1).encode_to_vec());
        assert_eq!(
            request(&grouped, Encoding::Protobuf).err(),
            Some(DecodeError::TooManySpans(BATCH_SPANS_MAX + 1))
        );
        for refused in [vec![START, 8, 1], vec![END], vec![START; 1_000_000]] {
            assert!(
                matches!(
                    request(&refused, Encoding::Protobuf),
                    Err(DecodeError::Invalid(_))
                ),
                "{} bytes",
                refused.len()
            );
        }
    }

    /// A body that stops reading as the request part way is refused before
    /// the spans ahead of the fault are built.
    #[test]
    fn a_body_the_count_cannot_read_is_never_decoded() {
        let trailing = format!(
            r#"{{"resourceSpans":[{{"scopeSpans":[{{"spans":[{}]}}]}}]}} and then some"#,
            ["{}"; 10].join(",")
        );
        let refused = request(trailing.as_bytes(), Encoding::Json).unwrap_err();
        assert!(matches!(refused, DecodeError::Invalid(_)), "{refused:?}");
    }
}
