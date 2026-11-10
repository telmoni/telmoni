//! Every span of one export, read under the key's project into rows of
//! `spans` and a report of what was lost on the way.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;

use telmoni_shared::ProjectId;

use crate::contract::{BATCH_SPANS_MAX, CLOCK_DRIFT_MAX_SECS};
use crate::ingest::report::Report;
use crate::ingest::span::{self, Context};
use crate::store::SpanRow;

/// Why an export was refused whole, before any span of it was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchError {
    /// More spans than [`BATCH_SPANS_MAX`]: how many.
    TooManySpans(usize),
}

/// What an export came to: its rows, the content they keep, and what it lost.
#[derive(Debug, Default)]
pub struct Batch {
    /// The spans kept, in the order they came.
    pub rows: Vec<SpanRow>,
    /// The content of each kept span that carries any, on a project that keeps it.
    pub content: Vec<SpanContent>,
    /// Another exporter's spans, each with its content, held until its
    /// trace's tree names its agent ([`crate::foreign`]).
    pub held: Vec<(SpanRow, Vec<(String, String)>)>,
    /// The scores the spans' evaluation events gave, each naming its span.
    pub scores: Vec<crate::scores::ScoreRow>,
    /// The spans refused and the attributes dropped.
    pub report: Report,
}

/// One kept span's content: each field by its name, as it arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanContent {
    pub trace_id: String,
    pub span_id: String,
    pub fields: Vec<(String, String)>,
}

/// The sender's clock as its export left, by its [`crate::contract::SENT_AT`],
/// and ours as the export arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenderClock {
    pub sent_at: DateTime<Utc>,
    pub arrived: DateTime<Utc>,
}

/// Every span of `request`, read as the key's `project_id`'s, received at
/// `received_at`, each `user_id` and `session_id` stored as `store_id` says, its content
/// kept where `keep_content`; `sender` is the sender's clock, where its export
/// names it.
///
/// # Errors
///
/// The export holds more spans than the batch allows; nothing of it is
/// read.
pub fn read(
    request: &ExportTraceServiceRequest,
    project_id: &ProjectId,
    received_at: DateTime<Utc>,
    sender: Option<SenderClock>,
    store_id: &dyn Fn(&str) -> String,
    keep_content: bool,
    scrub: crate::handler::privacy::DoorScrub,
) -> Result<Batch, BatchError> {
    let count: usize = request
        .resource_spans
        .iter()
        .flat_map(|resource| resource.scope_spans.iter())
        .map(|scope| scope.spans.len())
        .sum();
    if count > BATCH_SPANS_MAX {
        return Err(BatchError::TooManySpans(count));
    }

    let mut batch = Batch {
        rows: Vec::with_capacity(count),
        ..Batch::default()
    };
    let mut resources = Vec::with_capacity(request.resource_spans.len());
    for resource_spans in &request.resource_spans {
        let first = batch.rows.len();
        let sdk_resource = resource_spans.resource.as_ref().is_some_and(|resource| {
            resource.attributes.iter().any(|attribute| {
                attribute.key == crate::contract::DISTRO_NAME
                    && matches!(
                        attribute.value.as_ref().and_then(|value| value.value.as_ref()),
                        Some(Value::StringValue(name)) if name == "telmoni"
                    )
            })
        });
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
                resource_sent: &resource.sent,
                store_id,
                keep_content,
            };
            for one in &scope_spans.spans {
                // A span refused whole is named once, by its refusal: what
                // was dropped from it before the refusal was known is not
                // reported on top, since none of it was going to be kept.
                let dropped_before = batch.report.dropped.len();
                match span::map(one, &context, &mut batch.report.dropped) {
                    Ok((mut row, mut fields)) => {
                        if !sdk_resource {
                            // Another distribution's own counts are a claim: only the door's stand.
                            if let Some(at) = row
                                .attributes
                                .iter()
                                .position(|(key, _)| key == crate::contract::SCRUBBED)
                            {
                                row.attributes.remove(at);
                                row.sent.retain(|key| key != crate::contract::SCRUBBED);
                                row.units = span::units_of(&row);
                            }
                            scrub_fields(&mut row, &mut fields, scrub);
                        }
                        batch
                            .scores
                            .extend(scores_of(one, &row, project_id, received_at));
                        if !fields.is_empty() {
                            batch.content.push(SpanContent {
                                trace_id: row.trace_id.clone(),
                                span_id: row.span_id.clone(),
                                fields,
                            });
                        }
                        batch.rows.push(row);
                    }
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
    hold_foreign(&mut batch);
    Ok(batch)
}

/// Before foreign staging too, so the earliest durable copy is already scrubbed.
fn scrub_fields(
    row: &mut SpanRow,
    fields: &mut Vec<(String, String)>,
    policy: crate::handler::privacy::DoorScrub,
) {
    let switches = telmoni::scrub::Switches {
        secrets: policy.door_scrub_secrets,
        pii: policy.door_scrub_pii,
    };
    if !switches.any() || fields.is_empty() {
        return;
    }
    let mut counts = telmoni::scrub::Counts::new();
    fields.retain_mut(
        |(field, text)| match telmoni::scrub::scrub(text, switches) {
            Some((scrubbed, found)) => {
                *text = scrubbed;
                for (kind, count) in found {
                    *counts.entry(kind).or_default() += count;
                }
                true
            }
            None => {
                *counts.entry(format!("dropped:{field}")).or_default() += 1;
                false
            }
        },
    );
    if !counts.is_empty() {
        row.attributes.push((
            crate::contract::SCRUBBED.into(),
            serde_json::json!(counts).to_string(),
        ));
        if !row.sent.iter().any(|key| key == crate::contract::SCRUBBED) {
            row.sent.push(crate::contract::SCRUBBED.into());
        }
        row.units = span::units_of(row);
    }
}

/// A kept span's scores, from its evaluation events: the agent's own span's
/// score the trace, any other span itself.
fn scores_of(
    span: &opentelemetry_proto::tonic::trace::v1::Span,
    row: &SpanRow,
    project_id: &ProjectId,
    received_at: DateTime<Utc>,
) -> Vec<crate::scores::ScoreRow> {
    use telmoni_shared::{ScoreSource, ScoreTarget, SpanKind};
    crate::scores::of_span(span)
        .into_iter()
        .map(|event| {
            let score_id =
                crate::scores::sdk_score_id(project_id, &row.trace_id, &row.span_id, &event);
            let judgement = event.judgement.into_inner();
            crate::scores::ScoreRow {
                project_id: project_id.clone(),
                received_at,
                score_id,
                target: if row.kind == SpanKind::Agent {
                    ScoreTarget::Trace
                } else {
                    ScoreTarget::Span
                },
                trace_id: row.trace_id.clone(),
                span_id: row.span_id.clone(),
                session_id: row.session_id.clone(),
                value: judgement.stored_value(),
                name: judgement.name,
                data_type: judgement.data_type,
                categories: judgement.categories,
                range_min: judgement.range_min,
                range_max: judgement.range_max,
                source: ScoreSource::Sdk,
                scorer: String::new(),
                scorer_version: event.version,
                is_deleted: 0,
            }
        })
        .collect()
}

/// Every span naming no agent, another exporter's, moved to `held` with its
/// content, once its clock is read as its export's.
fn hold_foreign(batch: &mut Batch) {
    if batch.rows.iter().all(|row| !row.agent.is_empty()) {
        return;
    }
    let (held, rows): (Vec<SpanRow>, Vec<SpanRow>) = std::mem::take(&mut batch.rows)
        .into_iter()
        .partition(|row| row.agent.is_empty());
    batch.rows = rows;
    let theirs_ids: std::collections::HashSet<(String, String)> = held
        .iter()
        .map(|row| (row.trace_id.clone(), row.span_id.clone()))
        .collect();
    let (mut theirs, ours): (Vec<SpanContent>, Vec<SpanContent>) =
        std::mem::take(&mut batch.content)
            .into_iter()
            .partition(|content| {
                theirs_ids.contains(&(content.trace_id.clone(), content.span_id.clone()))
            });
    batch.content = ours;
    batch.held = held
        .into_iter()
        .map(|row| {
            let fields = theirs
                .iter()
                .position(|c| c.trace_id == row.trace_id && c.span_id == row.span_id)
                .map(|at| theirs.swap_remove(at).fields)
                .unwrap_or_default();
            (row, fields)
        })
        .collect();
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

    /// A span of ours, inside a trace.
    fn ours(n: u8, operation: Option<&str>) -> Span {
        let mut attributes = vec![kv(contract::AGENT, "nightly-report")];
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
    fn door_scrubbing_matches_the_corpus_before_foreign_staging() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../sdk/fixtures/canary.json"))
                .expect("the corpus");
        let project = ProjectId::new();
        for case in fixture["cases"].as_array().expect("cases") {
            for (mode, switches) in fixture["modes"].as_object().expect("modes") {
                let policy = crate::handler::privacy::DoorScrub {
                    door_scrub_secrets: switches["scrub_secrets"].as_bool().expect("switch"),
                    door_scrub_pii: switches["scrub_pii"].as_bool().expect("switch"),
                };
                let mut one = ours(1, None);
                one.attributes = vec![kv(
                    "gen_ai.system_instructions",
                    case["text"].as_str().expect("text"),
                )];
                let request = ExportTraceServiceRequest {
                    resource_spans: vec![ResourceSpans {
                        scope_spans: vec![scoped("foreign", vec![one])],
                        ..ResourceSpans::default()
                    }],
                };
                let batch = read(
                    &request,
                    &project,
                    Utc::now(),
                    None,
                    &|id| id.into(),
                    true,
                    policy,
                )
                .expect("batch");
                assert!(batch.rows.is_empty());
                assert_eq!(batch.held.len(), 1);
                let (row, fields) = &batch.held[0];
                assert_eq!(
                    fields[0].1,
                    case["expect"][mode]["text"].as_str().expect("expected"),
                    "{} {mode}",
                    case["name"]
                );
                let counts = row
                    .attributes
                    .iter()
                    .find(|(key, _)| key == contract::SCRUBBED)
                    .map_or_else(
                        || serde_json::json!({}),
                        |(_, value)| {
                            serde_json::from_str::<serde_json::Value>(value).expect("counts")
                        },
                    );
                assert_eq!(counts, case["expect"][mode]["scrubbed"]);
            }
        }
    }

    /// Another distribution's own `telmoni.scrubbed` is dropped, found or not, switches on or off.
    #[test]
    fn a_foreign_spans_own_scrub_counts_are_never_kept() {
        let mut one = ours(1, None);
        one.attributes = vec![
            kv("gen_ai.system_instructions", "nothing to find here"),
            kv(contract::SCRUBBED, r#"{"secret:forged":9}"#),
        ];
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![scoped("foreign", vec![one])],
                ..ResourceSpans::default()
            }],
        };
        for on in [true, false] {
            let policy = crate::handler::privacy::DoorScrub {
                door_scrub_secrets: on,
                door_scrub_pii: on,
            };
            let batch = read(
                &request,
                &ProjectId::new(),
                Utc::now(),
                None,
                &|id| id.into(),
                true,
                policy,
            )
            .expect("batch");
            let (row, _) = &batch.held[0];
            assert!(
                row.attributes
                    .iter()
                    .all(|(key, _)| key != contract::SCRUBBED),
                "switches {on}"
            );
            assert!(row.sent.iter().all(|key| key != contract::SCRUBBED));
        }
    }

    #[test]
    fn mixed_resources_keep_sdk_content_and_obey_disabled_switches() {
        let text = "email: person@example.com password=very-private-password";
        let mut one = ours(1, Some("invoke_agent"));
        one.attributes.push(kv("gen_ai.system_instructions", text));
        let request = ExportTraceServiceRequest {
            resource_spans: vec![
                ResourceSpans {
                    resource: Some(Resource {
                        attributes: vec![kv(contract::DISTRO_NAME, "telmoni")],
                        ..Resource::default()
                    }),
                    scope_spans: vec![scoped(contract::SCOPE, vec![one.clone()])],
                    ..ResourceSpans::default()
                },
                ResourceSpans {
                    scope_spans: vec![scoped(
                        contract::SCOPE,
                        vec![Span {
                            span_id: vec![2; 8],
                            ..one
                        }],
                    )],
                    ..ResourceSpans::default()
                },
            ],
        };
        let project = ProjectId::new();
        let policy = crate::handler::privacy::DoorScrub {
            door_scrub_secrets: true,
            door_scrub_pii: true,
        };
        let batch = read(
            &request,
            &project,
            Utc::now(),
            None,
            &|id| id.into(),
            true,
            policy,
        )
        .expect("batch");
        assert_eq!(batch.content[0].fields[0].1, text);
        assert!(!batch.content[1].fields[0].1.contains("person@example.com"));
        assert!(
            !batch.content[1].fields[0]
                .1
                .contains("very-private-password")
        );
        let off = crate::handler::privacy::DoorScrub {
            door_scrub_secrets: false,
            door_scrub_pii: false,
        };
        let pii_only = read(
            &request,
            &project,
            Utc::now(),
            None,
            &|id| id.into(),
            true,
            crate::handler::privacy::DoorScrub {
                door_scrub_secrets: false,
                door_scrub_pii: true,
            },
        )
        .expect("personal data only");
        assert!(
            pii_only.content[1].fields[0]
                .1
                .contains("very-private-password")
        );
        assert!(
            !pii_only.content[1].fields[0]
                .1
                .contains("person@example.com")
        );
        let batch = read(
            &request,
            &project,
            Utc::now(),
            None,
            &|id| id.into(),
            true,
            off,
        )
        .expect("batch");
        assert!(
            batch
                .content
                .iter()
                .all(|content| content.fields[0].1 == text)
        );
        let batch = read(
            &request,
            &project,
            Utc::now(),
            None,
            &|id| id.into(),
            false,
            policy,
        )
        .expect("batch");
        assert!(batch.content.is_empty());
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
        let batch = read(
            &request,
            &project,
            now,
            None,
            &keep,
            false,
            Default::default(),
        )
        .expect("spans");

        let kinds: Vec<(String, SpanKind)> = batch
            .rows
            .iter()
            .map(|row| (row.span_id.clone(), row.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("01".repeat(8), SpanKind::Agent),
                ("02".repeat(8), SpanKind::Step),
                ("03".repeat(8), SpanKind::Span),
            ],
            "the foreign span is held"
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
        assert!(batch.report.refused.is_empty());
        let held: Vec<&str> = batch
            .held
            .iter()
            .map(|(row, _)| row.span_id.as_str())
            .collect();
        assert_eq!(
            held,
            ["05".repeat(8)],
            "another exporter's span waits for its tree"
        );
        let dropped: Vec<(&str, &str)> = batch
            .report
            .dropped
            .named()
            .iter()
            .map(|d| (d.span_id.as_str(), d.attribute.as_str()))
            .collect();
        assert_eq!(
            dropped,
            [("0505050505050505", "vendor.prompt")],
            "kept, its unlisted text is dropped as any span's"
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
            read(
                &request,
                &project,
                received_at,
                None,
                &keep,
                false,
                Default::default(),
            )
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
            read(
                request,
                &project,
                arrived,
                sender,
                &keep,
                false,
                Default::default(),
            )
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
        // Refused once its attributes are read: it ends before it starts.
        let refused = {
            let span = ours(2, None);
            Span {
                end_time_unix_nano: span.start_time_unix_nano - 1,
                ..span
            }
        };
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![scoped(
                    contract::SCOPE,
                    vec![
                        with_unlisted(ours(1, None), 0, DROPS_NAMED_MAX - 3),
                        with_unlisted(refused, 0, 7),
                        with_unlisted(ours(3, None), 0, 5),
                    ],
                )],
                ..ResourceSpans::default()
            }],
        };
        let keep = |id: &str| id.to_owned();
        let batch = read(
            &request,
            &ProjectId::new(),
            Utc::now(),
            None,
            &keep,
            false,
            Default::default(),
        )
        .expect("spans");
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
    fn an_export_of_too_many_spans_is_refused_whole() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
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
            read(
                &too_many,
                &project,
                Utc::now(),
                None,
                &keep,
                false,
                Default::default()
            )
            .unwrap_err(),
            BatchError::TooManySpans(BATCH_SPANS_MAX + 1)
        );
    }
}
