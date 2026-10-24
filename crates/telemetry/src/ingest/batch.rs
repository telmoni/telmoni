//! Every span of one export, read under the key's project into rows of
//! `spans` and a report of what was lost on the way.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;

use telmoni_shared::ProjectId;

use crate::contract::{BATCH_SPANS_MAX, CLOCK_DRIFT_MAX_SECS};
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

/// The sender's clock as its export left, by its [`crate::contract::SENT_AT`],
/// and ours as the export arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenderClock {
    pub sent_at: DateTime<Utc>,
    pub arrived: DateTime<Utc>,
}

/// Every span of `request`, read as the key's `project_id`'s, received at
/// `received_at`, each `user_id` stored as `store_user_id` says; `sender` is
/// the sender's clock, where its export names it.
///
/// # Errors
///
/// The export holds no span, or more than the batch allows; nothing of it
/// is read.
pub fn read(
    request: &ExportTraceServiceRequest,
    project_id: &ProjectId,
    received_at: DateTime<Utc>,
    sender: Option<SenderClock>,
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
    let mut resources = Vec::with_capacity(request.resource_spans.len());
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
                    Ok(row) => batch.rows.push(row),
                    Err(refused) => {
                        batch.report.dropped.truncate(dropped_before);
                        batch.report.refused.push(refused);
                    }
                }
            }
        }
        resources.push(first..batch.rows.len());
    }
    // Believed only where no span ends a drift past it: a span is sent once ended.
    let allowed = chrono::Duration::seconds(CLOCK_DRIFT_MAX_SECS);
    let believed = sender.filter(|clock| {
        batch
            .rows
            .iter()
            .all(|row| row.ended_at - clock.sent_at <= allowed)
    });
    match believed {
        Some(clock) => {
            let ahead = clock.sent_at - clock.arrived;
            if ahead.abs() > allowed {
                for row in &mut batch.rows {
                    row.started_at -= ahead;
                    row.ended_at -= ahead;
                }
            }
        }
        // One resource is one process, so one clock.
        None => {
            for range in resources {
                if let Some(rows) = batch.rows.get_mut(range) {
                    arrived(rows, received_at);
                }
            }
        }
    }
    Ok(batch)
}

/// Where no clock is believed, a trace ending over [`CLOCK_DRIFT_MAX_SECS`] past arrival
/// moves back to end then, as one: spans are sent once ended, so its clock is ahead.
fn arrived(rows: &mut [SpanRow], received_at: DateTime<Utc>) {
    let mut last_end: HashMap<&str, DateTime<Utc>> = HashMap::new();
    for row in &*rows {
        let end = last_end.entry(&row.trace_id).or_insert(row.ended_at);
        *end = (*end).max(row.ended_at);
    }
    let allowed = chrono::Duration::seconds(CLOCK_DRIFT_MAX_SECS);
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
    use crate::ingest::report::DROPS_NAMED_MAX;

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
                        vec![Span {
                            attributes: vec![kv("vendor.prompt", "the prompt")],
                            ..ours(5, None)
                        }],
                    )],
                    ..ResourceSpans::default()
                },
            ],
        };
        let project = ProjectId::new();
        let now = Utc::now().trunc_subsecs(6);
        let keep = |id: &str| id.to_owned();
        let batch = read(&request, &project, now, None, &keep).expect("spans");

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
            "the foreign span is refused"
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
            read(&request, &project, received_at, None, &keep)
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
        let allowed = chrono::Duration::seconds(CLOCK_DRIFT_MAX_SECS);
        assert_eq!(times(at(SENT + 300_000_000) - allowed), sent);
    }

    /// A sender's clock past the drift moves its whole export, unless a span ends a drift past it.
    #[test]
    fn a_senders_clock_moves_its_whole_export_alike() {
        const SECOND: u64 = 1_000_000_000;
        const SENT: u64 = 1_760_000_000 * SECOND;
        const DAYS_400: u64 = 400 * 24 * 60 * 60 * SECOND;
        let timed = |n: u8, trace: u8, start: u64, end: u64| Span {
            trace_id: vec![trace; 16],
            start_time_unix_nano: start,
            end_time_unix_nano: end,
            ..ours(n, None)
        };
        // Two resources, timed by the clock that sent them, one end a typo or not.
        let export = |last_end: u64| ExportTraceServiceRequest {
            resource_spans: vec![
                ResourceSpans {
                    scope_spans: vec![scoped(
                        contract::SCOPE,
                        vec![
                            timed(1, 0x4b, SENT - 3 * SECOND, SENT - 2 * SECOND),
                            timed(2, 0x4b, SENT - 3 * SECOND, last_end),
                        ],
                    )],
                    ..ResourceSpans::default()
                },
                ResourceSpans {
                    scope_spans: vec![scoped(
                        contract::SCOPE,
                        vec![timed(3, 0x5c, SENT - 2 * SECOND, SENT - SECOND)],
                    )],
                    ..ResourceSpans::default()
                },
            ],
        };
        let at = |nanos: u64| DateTime::from_timestamp_nanos(i64::try_from(nanos).expect("an i64"));
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        // As sent from a clock `ahead` of ours, or with no clock named.
        let times = |request: &ExportTraceServiceRequest,
                     sent_at: DateTime<Utc>,
                     ahead: chrono::Duration,
                     named: bool|
         -> Vec<_> {
            let arrived = at(SENT) - ahead;
            let sender = named.then_some(SenderClock { sent_at, arrived });
            read(request, &project, arrived, sender, &keep)
                .expect("spans")
                .rows
                .iter()
                .map(|row| (row.started_at, row.ended_at))
                .collect()
        };

        let request = export(SENT - SECOND);
        let sent = times(&request, at(SENT), chrono::Duration::zero(), false);
        let allowed = chrono::Duration::seconds(CLOCK_DRIFT_MAX_SECS);
        let nanosecond = chrono::Duration::nanoseconds(1);
        for ahead in [
            chrono::Duration::hours(3),
            -chrono::Duration::days(1),
            allowed + nanosecond,
            -allowed - nanosecond,
        ] {
            let moved: Vec<_> = sent
                .iter()
                .map(|(start, end)| (*start - ahead, *end - ahead))
                .collect();
            assert_eq!(times(&request, at(SENT), ahead, true), moved, "{ahead}");
        }
        for ahead in [allowed, -allowed] {
            assert_eq!(times(&request, at(SENT), ahead, true), sent, "{ahead}");
        }

        let ahead = chrono::Duration::hours(3);
        let typo = export(SENT + DAYS_400);
        assert_eq!(
            times(&typo, at(SENT), ahead, true),
            times(&typo, at(SENT), ahead, false),
            "a span ending past the clock, which cannot have timed it: as none"
        );
        let in_milliseconds = at(SENT / 1_000_000);
        assert_eq!(
            times(&request, in_milliseconds, ahead, true),
            times(&request, in_milliseconds, ahead, false),
            "a clock in another unit, ended past by every span: as none"
        );
    }

    /// A refused span's drops roll back across the names kept, the next span's taking them.
    #[test]
    fn a_refused_spans_drops_are_rolled_back_across_the_names_kept() {
        let with_unlisted = |span: Span, from: usize, count: usize| Span {
            attributes: span
                .attributes
                .into_iter()
                .chain((from..from + count).map(|i| kv(&format!("vendor.x{i}"), "v")))
                .collect(),
            ..span
        };
        let not_a_run = Span {
            attributes: Vec::new(),
            ..ours(2, None)
        };
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![scoped(
                    contract::SCOPE,
                    vec![
                        with_unlisted(ours(1, None), 0, DROPS_NAMED_MAX - 3),
                        with_unlisted(not_a_run, 0, 7),
                        with_unlisted(ours(3, None), 0, 5),
                    ],
                )],
                ..ResourceSpans::default()
            }],
        };
        let keep = |id: &str| id.to_owned();
        let batch = read(&request, &ProjectId::new(), Utc::now(), None, &keep).expect("spans");
        assert_eq!(batch.report.refused.len(), 1);
        let dropped = &batch.report.dropped;
        assert_eq!(
            (dropped.named().len(), dropped.len()),
            (DROPS_NAMED_MAX, DROPS_NAMED_MAX + 2)
        );
        let spans: Vec<&str> = dropped
            .named()
            .iter()
            .skip(DROPS_NAMED_MAX - 3)
            .map(|d| d.span_id.as_str())
            .collect();
        assert_eq!(spans, vec!["03".repeat(8); 3]);
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
            read(&empty, &project, Utc::now(), None, &keep).unwrap_err(),
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
            read(&too_many, &project, Utc::now(), None, &keep).unwrap_err(),
            BatchError::TooManySpans(BATCH_SPANS_MAX + 1)
        );
    }
}
