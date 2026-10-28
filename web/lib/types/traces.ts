import { z } from "zod";

import { SpanKind, SpanStatus } from "@/lib/types/enums";

// A trace as the server answers it, parsed on the server by its entity
// (`lib/server/entities/traces.ts`) and in the browser by the peeks.

// One trace of the list: a run's spans summed by the server, which reads the
// project's spans and no other's. `user_id` is as stored — hashed, unless
// the project keeps it plain — and `ended` says whether the run's own span has
// arrived; until it has, `status` is read off the spans that have.
export const TraceSchema = z.object({
  run_id: z.string(),
  agent: z.string(),
  user_id: z.string(),
  session_id: z.string(),
  ended: z.boolean(),
  status: z.enum(SpanStatus),
  /** The class of the error the run's own span ended on, never its
   *  message; empty otherwise. */
  error_type: z.string(),
  started_at: z.string(),
  ended_at: z.string(),
  duration_ms: z.number(),
  spans: z.number(),
  steps: z.number(),
  model_calls: z.number(),
  tool_calls: z.number(),
  errors: z.number(),
  input_tokens: z.number(),
  output_tokens: z.number(),
  cache_read_input_tokens: z.number(),
  cache_write_input_tokens: z.number(),
  models: z.array(z.string()),
  tools: z.array(z.string()),
  /** The deployments its spans were sent from, each once: one, unless the
   *  run crossed services deployed apart. */
  environments: z.array(z.string()),
  /** The releases its spans were sent from, each once. */
  releases: z.array(z.string()),
});
export type Trace = z.infer<typeof TraceSchema>;

// One span of a trace, as the server answers it: the row with its maps as
// objects and its times as text.
export const TraceSpanSchema = z.object({
  span_id: z.string(),
  parent_span_id: z.string(),
  trace_id: z.string(),
  agent: z.string(),
  kind: z.enum(SpanKind),
  name: z.string(),
  started_at: z.string(),
  ended_at: z.string(),
  status: z.enum(SpanStatus),
  error_type: z.string(),
  model: z.string(),
  provider: z.string(),
  input_tokens: z.number(),
  output_tokens: z.number(),
  cache_read_input_tokens: z.number(),
  cache_write_input_tokens: z.number(),
  /** What it cost in US dollars, as reported; null where it reported none. */
  cost: z.number().nullable(),
  user_id: z.string(),
  session_id: z.string(),
  /** The deployment its resource named, and its release; empty for none. */
  environment: z.string(),
  release: z.string(),
  tags: z.record(z.string(), z.string()),
  sample_rate: z.number().nullable(),
  attributes: z.record(z.string(), z.string()),
  units: z.number(),
  received_at: z.string(),
});
export type TraceSpan = z.infer<typeof TraceSpanSchema>;

/** What a run has spent, as an alert on its cost reads it: each call's cost
 *  as reported, else its tokens at Telmoni's price, and the models priced by
 *  neither, which it leaves out. */
export const RunSpendSchema = z.object({
  spent: z.number(),
  unpriced: z.array(z.object({ provider: z.string(), model: z.string() })),
});
export type RunSpend = z.infer<typeof RunSpendSchema>;

export const TraceDetailSchema = z.object({
  run_id: z.string(),
  spans: z.array(TraceSpanSchema),
  truncated: z.boolean(),
  /** None where the server could not read it. */
  spend: RunSpendSchema.nullable(),
});
export type TraceDetail = z.infer<typeof TraceDetailSchema>;
