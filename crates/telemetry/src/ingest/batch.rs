//! Every span of one export, read under the key's project into rows of
//! `spans` and a report of what was lost on the way.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;

use telmoni_shared::ProjectId;

use crate::contract::{BATCH_SPANS_MAX, CLOCK_AHEAD_MAX_SECS};
use crate::ingest::report::Report;
use crate::ingest::span::{self, Context};
use crate::store::SpanRow;

/// Why an export was refused whole, before any span of it was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchError {
    /// Not one span in it.
    NoSpans,
    /// More spans than [`BATCH_SPANS_MAX`]: how many.
    TooManySpans(usize),
}

/// What an export came to: its rows, and what it lost.
#[derive(Debug, Default)]
pub struct Batch {
    /// The spans kept, in the order they came.
    pub rows: Vec<SpanRow>,
    /// The spans refused and the attributes dropped.
    pub report: Report,
}

/// Every span of `request`, read as the key's `project_id`'s, received at
/// `received_at`, each `user_id` stored as `store_user_id` says.
///
/// # Errors
///
/// The export holds no span, or more than the batch allows; nothing of it
/// is read.
pub fn read(
    request: &ExportTraceServiceRequest,
    project_id: &ProjectId,
    received_at: DateTime<Utc>,
    store_user_id: &dyn Fn(&str) -> String,
) -> Result<Batch, BatchError> {
    let count: usize = request
        .resource_spans
        .iter()
        .flat_map(|resource| resource.scope_spans.iter())
        .map(|scope| scope.spans.len())
        .sum();
    if count == 0 {
        return Err(BatchError::NoSpans);
    }
    if count > BATCH_SPANS_MAX {
        return Err(BatchError::TooManySpans(count));
    }

    let mut batch = Batch::default();
    for resource_spans in &request.resource_spans {
        let first = batch.rows.len();
        let resource = span::read_resource(resource_spans.resource.as_ref());
        for scope_spans in &resource_spans.scope_spans {
            let context = Context {
                project_id,
                received_at,
                scope: scope_spans
                    .scope
                    .as_ref()
                    .map_or("", |scope| scope.name.as_str()),
                environment: &resource.environment,
                release: &resource.release,
                resource: &resource.attributes,
                store_user_id,
            };
            for one in &scope_spans.spans {
                // A span refused whole is named once, by its refusal: what
                // was dropped from it before the refusal was known is not
                // reported on top, since none of it was going to be kept.
                let dropped_before = batch.report.dropped.len();
                match span::map(one, &context, &mut batch.report.dropped) {
                    Ok(Some(row)) => batch.rows.push(row),
                    Ok(None) => {}
                    Err(refused) => {
                        batch.report.dropped.truncate(dropped_before);
                        batch.report.refused.push(refused);
                    }
                }
            }
        }
        // One resource is one process, so one clock.
        if let Some(rows) = batch.rows.get_mut(first..) {
            arrived(rows, received_at);
        }
    }
    Ok(batch)
}

/// Each trace among `rows` ending past [`CLOCK_AHEAD_MAX_SECS`] after arrival,
/// moved back as one to end then: spans are sent once ended, so its clock is ahead.
fn arrived(rows: &mut [SpanRow], received_at: DateTime<Utc>) {
    let mut last_end: HashMap<&str, DateTime<Utc>> = HashMap::new();
    for row in &*rows {
        let end = last_end.entry(&row.trace_id).or_insert(row.ended_at);
        *end = (*end).max(row.ended_at);
    }
    let allowed = chrono::Duration::seconds(CLOCK_AHEAD_MAX_SECS);
    let moves: Vec<Option<chrono::Duration>> = rows
        .iter()
        .map(|row| {
            last_end
                .get(row.trace_id.as_str())
                .map(|end| *end - received_at)
                .filter(|ahead| *ahead > allowed)
        })
        .collect();
    for (row, ahead) in rows.iter_mut().zip(moves) {
        if let Some(ahead) = ahead {
            row.started_at -= ahead;
            row.ended_at -= ahead;
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::SubsecRound as _;
    use opentelemetry_proto::tonic::common::v1::{
        AnyValue, InstrumentationScope, KeyValue, any_value::Value,
    };
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
    use telmoni_shared::SpanKind;

    use super::*;
    use crate::contract;

    fn kv(key: &str, text: &str) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue {
                value: Some(Value::StringValue(text.into())),
            }),
            ..KeyValue::default()
        }
    }

    /// A span of ours, inside a run.
    fn ours(n: u8, operation: Option<&str>) -> Span {
        let mut attributes = vec![
            kv(contract::RUN_ID, "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01"),
            kv(contract::AGENT, "nightly-report"),
        ];
        if let Some(operation) = operation {
            attributes.push(kv(contract::OPERATION, operation));
        }
        Span {
            trace_id: vec![0x4b; 16],
            span_id: vec![n; 8],
            name: format!("span {n}"),
            start_time_unix_nano: 1_760_000_000_000_000_000,
            end_time_unix_nano: 1_760_000_000_100_000_000,
            attributes,
            ..Span::default()
        }
    }

    fn scoped(name: &str, spans: Vec<Span>) -> ScopeSpans {
        ScopeSpans {
            scope: Some(InstrumentationScope {
                name: name.into(),
                ..InstrumentationScope::default()
            }),
            spans,
            ..ScopeSpans::default()
        }
    }

    #[test]
    fn every_span_is_read_under_its_resource_and_scope() {
        let request = ExportTraceServiceRequest {
            resource_spans: vec![
                ResourceSpans {
                    resource: Some(Resource {
                        attributes: vec![
                            kv("service.name", "reporter"),
                            kv("host.name", "laptop"),
                            kv(contract::ENVIRONMENT, "staging"),
                        ],
                        ..Resource::default()
                    }),
                    scope_spans: vec![
                        scoped(
                            contract::SCOPE,
                            vec![ours(1, Some("invoke_agent")), ours(2, None)],
                        ),
                        scoped("opentelemetry.instrumentation.httpx", vec![ours(3, None)]),
                    ],
                    ..ResourceSpans::default()
                },
                ResourceSpans {
                    scope_spans: vec![scoped(
                        contract::SCOPE,
                        vec![
                            Span {
                                name: contract::RUN_START.into(),
                                ..ours(4, None)
                            },
                            Span {
                                attributes: vec![kv("vendor.prompt", "the prompt")],
                                ..ours(5, None)
                            },
                        ],
                    )],
                    ..ResourceSpans::default()
                },
            ],
        };
        let project = ProjectId::new();
        let now = Utc::now().trunc_subsecs(6);
        let keep = |id: &str| id.to_owned();
        let batch = read(&request, &project, now, &keep).expect("spans");

        let kinds: Vec<(String, SpanKind)> = batch
            .rows
            .iter()
            .map(|row| (row.span_id.clone(), row.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("01".repeat(8), SpanKind::Run),
                ("02".repeat(8), SpanKind::Step),
                ("03".repeat(8), SpanKind::Span),
            ],
            "the start is never a row, and the foreign span is refused"
        );
        assert!(batch.rows.iter().all(|row| row.received_at == now));
        assert_eq!(
            batch.rows[0].attributes,
            [("service.name".to_owned(), "reporter".to_owned())],
            "the first resource's inventory rides on its spans; host.name does not"
        );
        assert!(
            batch.rows[..3]
                .iter()
                .all(|row| row.environment == "staging" && row.release.is_empty()),
            "the first resource's deployment is on each of its spans"
        );
        assert!(
            batch.rows[2].attributes.is_empty() || batch.rows[2].attributes[0].0 == "service.name"
        );
        assert_eq!(batch.report.refused.len(), 1);
        assert_eq!(batch.report.refused[0].span_id, "05".repeat(8));
        assert!(
            batch.report.dropped.is_empty(),
            "{:?}",
            batch.report.dropped
        );
    }

    /// A trace from a clock ahead moves back as one; another trace, or another resource's, stays.
    #[test]
    fn a_trace_from_a_clock_ahead_is_moved_back_as_one() {
        const SECOND: u64 = 1_000_000_000;
        const SENT: u64 = 1_760_000_000 * SECOND;
        let arrival = SENT + 300_000_000 - 365 * 24 * 60 * 60 * SECOND;
        let timed = |n: u8, trace: u8, start: u64, end: u64| Span {
            trace_id: vec![trace; 16],
            start_time_unix_nano: start,
            end_time_unix_nano: end,
            ..ours(n, None)
        };
        let request = ExportTraceServiceRequest {
            resource_spans: vec![
                ResourceSpans {
                    scope_spans: vec![scoped(
                        contract::SCOPE,
                        vec![
                            ours(1, Some("invoke_agent")),
                            timed(2, 0x4b, SENT + 50_000_000, SENT + 300_000_000),
                            timed(3, 0x5c, arrival - SECOND, arrival - 700_000_000),
                        ],
                    )],
                    ..ResourceSpans::default()
                },
                ResourceSpans {
                    scope_spans: vec![scoped(
                        contract::SCOPE,
                        vec![timed(4, 0x4b, arrival - 500_000_000, arrival - 200_000_000)],
                    )],
                    ..ResourceSpans::default()
                },
            ],
        };
        let at = |nanos: u64| DateTime::from_timestamp_nanos(i64::try_from(nanos).expect("an i64"));
        let sent = [
            (at(SENT), at(SENT + 100_000_000)),
            (at(SENT + 50_000_000), at(SENT + 300_000_000)),
            (at(arrival - SECOND), at(arrival - 700_000_000)),
            (at(arrival - 500_000_000), at(arrival - 200_000_000)),
        ];
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let times = |received_at| -> Vec<_> {
            read(&request, &project, received_at, &keep)
                .expect("spans")
                .rows
                .iter()
                .map(|row| (row.started_at, row.ended_at))
                .collect()
        };

        let year = chrono::Duration::days(365);
        assert_eq!(
            times(at(arrival)),
            [
                (sent[0].0 - year, sent[0].1 - year),
                (sent[1].0 - year, at(arrival)),
                sent[2],
                sent[3],
            ]
        );
        let allowed = chrono::Duration::seconds(CLOCK_AHEAD_MAX_SECS);
        assert_eq!(times(at(SENT + 300_000_000) - allowed), sent);
    }

    #[test]
    fn an_export_with_no_span_or_too_many_is_refused_whole() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let empty = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![scoped(contract::SCOPE, Vec::new())],
                ..ResourceSpans::default()
            }],
        };
        assert_eq!(
            read(&empty, &project, Utc::now(), &keep).unwrap_err(),
            BatchError::NoSpans
        );
        let too_many = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![scoped(
                    contract::SCOPE,
                    (0..=BATCH_SPANS_MAX).map(|_| ours(1, None)).collect(),
                )],
                ..ResourceSpans::default()
            }],
        };
        assert_eq!(
            read(&too_many, &project, Utc::now(), &keep).unwrap_err(),
            BatchError::TooManySpans(BATCH_SPANS_MAX + 1)
        );
    }
}
