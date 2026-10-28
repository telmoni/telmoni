import { z } from "zod";

import { SpanKind, SpanStatus } from "@/lib/types/enums";
import { TraceSchema } from "@/lib/types/traces";

// A session's page as the server answers it, parsed on the server by its
// entity (`lib/server/entities/sessions.ts`) and in the browser by the peek.

// One span a layout pinned by name, shown under a trace while it is shut:
// enough to read without opening the trace.
export const PinnedSpanSchema = z.object({
  span_id: z.string(),
  parent_span_id: z.string(),
  kind: z.enum(SpanKind),
  name: z.string(),
  started_at: z.string(),
  ended_at: z.string(),
  status: z.enum(SpanStatus),
  error_type: z.string(),
  model: z.string(),
  input_tokens: z.number(),
  output_tokens: z.number(),
});
export type PinnedSpan = z.infer<typeof PinnedSpanSchema>;

// One trace of a session's page: the list's sums, and the spans the layout
// pinned under it.
export const SessionTraceSchema = TraceSchema.extend({ pinned: z.array(PinnedSpanSchema) });
export type SessionTrace = z.infer<typeof SessionTraceSchema>;

// A session's own page: its traces in the order they ran, each summed as the
// trace list sums it — a replay in metadata, never a message — and the span
// names their pinned spans were read by.
export const SessionDetailSchema = z.object({
  session_id: z.string(),
  traces: z.array(SessionTraceSchema),
  pinned: z.array(z.string()),
  days: z.number(),
  limit: z.number(),
  more: z.boolean(),
});
export type SessionDetail = z.infer<typeof SessionDetailSchema>;
