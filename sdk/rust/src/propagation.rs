//! A run across services: the W3C `traceparent` of the span current inside a
//! run, and the run's id and agent as the W3C `baggage` members
//! `telmoni.run_id` and `telmoni.agent`, written into an outgoing request's
//! headers and read back from an incoming one's. The SDK's own helpers,
//! never a propagator set globally: an application's propagators are its
//! own.

use opentelemetry::Context;
use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::trace::{
    SpanContext, SpanId, TraceContextExt as _, TraceFlags, TraceId, TraceState,
};
use uuid::Uuid;

use crate::contract;
use crate::runs::RunInfo;

/// Headers `inject` writes and `extract` reads: anything that is both an
/// OpenTelemetry [`Injector`] and [`Extractor`], as a `HashMap<String,
/// String>` is; a request's own headers go through a wrapper that is both.
/// A name is matched as the carrier matches it: a `HashMap`, through
/// OpenTelemetry's own impl, lowercases it on both sides.
pub trait Carrier: Injector + Extractor {}

impl<T: Injector + Extractor + ?Sized> Carrier for T {}

/// What headers name: the span to continue under, and the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Continued {
    pub(crate) span_context: SpanContext,
    pub(crate) run_id: String,
    pub(crate) agent: String,
}

/// The run and the span `cx` is inside, written into `headers`; nothing
/// outside a run, or for a run sampled out.
pub(crate) fn inject(cx: &Context, headers: &mut dyn Carrier) {
    let Some(run) = cx.get::<RunInfo>() else {
        return;
    };
    if run.sampled_out {
        return;
    }
    let span = cx.span();
    let span_context = span.span_context();
    if !span_context.is_valid() {
        return;
    }
    write_headers(headers, span_context, &run.run_id, &run.agent);
}

/// `span` and the run as the next service reads them, beside the members
/// `headers` already hold.
pub(crate) fn write_headers(
    headers: &mut dyn Carrier,
    span: &SpanContext,
    run_id: &str,
    agent: &str,
) {
    let sampled = if span.is_sampled() { "01" } else { "00" };
    headers.set(
        contract::TRACEPARENT,
        format!("00-{}-{}-{sampled}", span.trace_id(), span.span_id()),
    );
    let mut members: Vec<String> = get(headers, contract::BAGGAGE)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|member| !member.is_empty() && !is_ours(member))
        .map(str::to_owned)
        .collect();
    members.push(format!("{}={run_id}", contract::RUN_ID));
    members.push(format!(
        "{}={}",
        contract::AGENT,
        encode_baggage_value(agent)
    ));
    headers.set(contract::BAGGAGE, members.join(","));
}

/// The run `headers` name, or `None` for headers that name none.
pub(crate) fn read_headers(headers: &dyn Carrier) -> Option<Continued> {
    let span_context = parse_traceparent(get(headers, contract::TRACEPARENT)?)?;
    let baggage = get(headers, contract::BAGGAGE).unwrap_or_default();
    let mut run_id = None;
    let mut agent = None;
    for member in baggage.split(',') {
        let Some((key, rest)) = member.split_once('=') else {
            continue;
        };
        let value = decode_baggage_value(rest.split(';').next().unwrap_or("").trim());
        match key.trim() {
            contract::RUN_ID => run_id = Some(value),
            contract::AGENT => agent = Some(value),
            _ => {}
        }
    }
    let run_id = run_id.filter(|id| Uuid::parse_str(id).is_ok())?;
    let agent = agent.filter(|agent| !agent.is_empty())?;
    Some(Continued {
        span_context,
        run_id: run_id.to_ascii_lowercase(),
        agent,
    })
}

/// A `traceparent` as the W3C spells it: a version that is not `ff`, a
/// trace id and a span id that are not zeros, and the flags; a version other
/// than `00` may be followed by more, which is ignored.
pub(crate) fn parse_traceparent(header: &str) -> Option<SpanContext> {
    let parts: Vec<&str> = header.trim().split('-').collect();
    let [version, trace_id, span_id, flags, ..] = parts.as_slice() else {
        return None;
    };
    if !is_hex(version, 2) || *version == "ff" {
        return None;
    }
    if *version == "00" && parts.len() != 4 {
        return None;
    }
    if !is_hex(trace_id, 32) || !is_hex(span_id, 16) || !is_hex(flags, 2) {
        return None;
    }
    let trace_id = TraceId::from_hex(trace_id).ok()?;
    let span_id = SpanId::from_hex(span_id).ok()?;
    if trace_id == TraceId::INVALID || span_id == SpanId::INVALID {
        return None;
    }
    let flags = u8::from_str_radix(flags, 16).ok()?;
    Some(SpanContext::new(
        trace_id,
        span_id,
        TraceFlags::new(flags) & TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    ))
}

/// A baggage value as the W3C writes it: the printable ASCII its grammar
/// allows as it is, and every other byte of the UTF-8 percent-encoded, so a
/// space, a comma or a name in another script travels whole.
pub(crate) fn encode_baggage_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if allowed_in_baggage(byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A baggage value read back: each `%XX` its byte, a `%` without two hex
/// digits kept as it is.
pub(crate) fn decode_baggage_value(value: &str) -> String {
    let mut out = Vec::with_capacity(value.len());
    let mut rest = value.as_bytes();
    while let Some((&first, after)) = rest.split_first() {
        let encoded = (first == b'%')
            .then(|| after.get(..2))
            .flatten()
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match encoded {
            Some(byte) => {
                out.push(byte);
                rest = after.get(2..).unwrap_or_default();
            }
            None => {
                out.push(first);
                rest = after;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The W3C's `baggage-octet`: printable ASCII but space, `"`, `,`, `;` and
/// `\`; and but `%`, which the grammar allows and a reader would decode.
fn allowed_in_baggage(byte: u8) -> bool {
    matches!(byte, 0x21 | 0x23..=0x24 | 0x26..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e)
}

fn is_ours(member: &str) -> bool {
    let key = member.split('=').next().unwrap_or("").trim();
    key == contract::RUN_ID || key == contract::AGENT
}

/// Lowercase hex of exactly `len` digits.
fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The header named `name`, as the carrier matches names.
fn get<'a>(headers: &'a dyn Carrier, name: &str) -> Option<&'a str> {
    headers.get(name)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn a_baggage_value_is_encoded_as_the_grammar_asks_and_read_back() {
        let value = "support bot, \"the\" one; é\\%";
        assert_eq!(
            encode_baggage_value(value),
            "support%20bot%2C%20%22the%22%20one%3B%20%C3%A9%5C%25"
        );
        assert_eq!(decode_baggage_value(&encode_baggage_value(value)), value);
        assert_eq!(decode_baggage_value("a%2"), "a%2");
        assert_eq!(decode_baggage_value("a%zz"), "a%zz");
        assert_eq!(decode_baggage_value("%c3%A9"), "é");
        let plain = "plain-bot_1.0~!#$&'()*+/:<=>?@[]^`{|}";
        assert_eq!(encode_baggage_value(plain), plain);
    }

    #[test]
    fn a_traceparent_is_parsed_as_the_grammar_asks() {
        let parsed =
            parse_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").unwrap();
        assert_eq!(
            parsed.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(parsed.span_id().to_string(), "00f067aa0ba902b7");
        assert!(parsed.is_sampled() && parsed.is_remote());
        let flagged =
            parse_traceparent(" 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-03 ").unwrap();
        assert!(flagged.is_sampled());
        let future =
            parse_traceparent("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00-more")
                .unwrap();
        assert!(!future.is_sampled());
        for refused in [
            "",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e473-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-1",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-0g",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00F067AA0BA902B7-01",
        ] {
            assert!(
                parse_traceparent(refused).is_none(),
                "{refused:?} was taken"
            );
        }
    }

    /// A `HashMap` carrier matches names as OpenTelemetry's own impl does,
    /// lowercased on both sides: what a request's headers are read into.
    #[test]
    fn a_hash_map_carrier_reads_names_lowercased() {
        let mut headers: HashMap<String, String> = HashMap::new();
        headers.set("Traceparent", "x".to_owned());
        assert_eq!(get(&headers, "traceparent"), Some("x"));
        assert_eq!(get(&headers, "TRACEPARENT"), Some("x"));
        assert_eq!(get(&headers, "baggage"), None);
    }
}
