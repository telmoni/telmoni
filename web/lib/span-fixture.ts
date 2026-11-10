import type { TraceSpan } from "@/lib/server/entities/traces";
import { SpanKind, SpanStatus } from "@/lib/types/enums";

// One span for the tests, so the shape the server answers is written once:
// a test names only what its case turns on.

/** A plain span of the nightly report's trace, named by its id, with every
 *  field `over` leaves out empty, zero or none. */
export function testSpan(over: Partial<TraceSpan> & Pick<TraceSpan, "span_id">): TraceSpan {
  return {
    fault: null,
    parent_span_id: "",
    trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
    agent: "nightly-report",
    kind: SpanKind.Span,
    name: over.span_id,
    started_at: "2026-10-11T03:00:00.000Z",
    ended_at: "2026-10-11T03:00:01.000Z",
    status: SpanStatus.Ok,
    error_type: "",
    model: "",
    provider: "",
    input_tokens: 0,
    output_tokens: 0,
    cache_read_input_tokens: 0,
    cache_write_input_tokens: 0,
    reasoning_output_tokens: 0,
    cost: null,
    time_to_first_token: null,
    output_speed: null,
    user_id: "",
    session_id: "",
    environment: "",
    release: "",
    tags: {},
    sample_rate: null,
    attributes: {},
    units: 1,
    received_at: "2026-10-11T03:00:02.000Z",
    ...over,
  };
}
