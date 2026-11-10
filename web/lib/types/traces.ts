import { z } from "zod";

import { ScoreType, SpanFault, SpanKind, SpanStatus } from "@/lib/types/enums";
import { ScoreSchema } from "@/lib/types/scores";

// A trace as the server answers it, parsed on the server by its entity
// (`lib/server/entities/traces.ts`) and in the browser by the peeks.

/** A column of the trace list a person shows or hides, by the word their
 *  browser keeps it under (`lib/trace-prefs.ts`). */
export const TraceColumn = {
  Started:          "started",
  Ended:            "ended",
  Agent:            "agent",
  User:             "user",
  Session:          "session",
  Environment:      "environment",
  Release:          "release",
  Status:           "status",
  ErrorType:        "error_type",
  Steps:            "steps",
  ModelCalls:       "model_calls",
  ToolCalls:        "tool_calls",
  Retrievals:       "retrievals",
  Errors:           "errors",
  Spans:            "spans",
  InputTokens:      "input_tokens",
  OutputTokens:     "output_tokens",
  CacheReadTokens:  "cache_read_tokens",
  CacheWriteTokens: "cache_write_tokens",
  Tokens:           "tokens",
  Spend:            "spend",
  Duration:         "duration",
  TimeToFirstToken: "time_to_first_token",
  Models:           "models",
  Tools:            "tools",
  Tags:             "tags",
  Scores:           "scores",
} as const;
export type TraceColumn = typeof TraceColumn[keyof typeof TraceColumn];

const Values = z.array(z.string()).optional();

/** What narrows a list as the URL carries it, in the sidebar's order: a
 *  filter is the values any of which admits a trace, a key left out narrowing
 *  nothing; a session or a user is admitted by a trace of theirs. The one list
 *  of the keys: the facets and every lane's query take them from here. */
export const ListFiltersSchema = z.object({
  agent: Values,
  status: z.array(z.enum(SpanStatus)).optional(),
  fault: z.array(z.enum(SpanFault)).optional(),
  model: Values,
  tool: Values,
  user: Values,
  session: Values,
  environment: Values,
  release: Values,
  /** Each `key:value`. */
  tag: Values,
  prompt: Values,
  /** Each `name:version`. */
  prompt_version: Values,
});
export type ListFilters = z.infer<typeof ListFiltersSchema>;
export type FilterKey = keyof ListFilters;
/** The filters' keys, in the sidebar's order. */
export const FILTER_KEYS = ListFiltersSchema.keyof().options;

/** `query`'s filters alone, every key present. */
export function filtersIn(query: ListFilters): ListFilters {
  return Object.fromEntries(FILTER_KEYS.map((key) => [key, query[key]])) as ListFilters;
}

/** The longest key and value a tag has, in characters, as ingest keeps one. */
export const TAG_KEY_MAX = 64;
export const TAG_VALUE_MAX = 256;

/** A sort by one tag's value, `tag:` and its key: the trace list takes one
 *  beside its columns' words. */
export type TagSort = `tag:${string}`;

/** `text` as a tag sort, as the server reads one: `tag:` and a key of 1 to
 *  64 characters, none a control character; else none. */
export function readTagSort(text: string): TagSort | undefined {
  if (!text.startsWith("tag:")) return undefined;
  const key = text.slice("tag:".length);
  const length = [...key].length;
  return length >= 1 && length <= TAG_KEY_MAX && !/\p{Cc}/u.test(key) ? `tag:${key}` : undefined;
}

/** The key a list is sorted by, where its sort is a tag's. */
export function tagSortKey(sort: string): string | undefined {
  return readTagSort(sort)?.slice("tag:".length);
}

/** A tag sort, as a list answers one. */
export const TagSortSchema = z.custom<TagSort>(
  (value) => typeof value === "string" && readTagSort(value) !== undefined,
);

// One score name of a trace or a session: its mean, how many, and a
// category's values, which have no mean.
export const ScoreSummarySchema = z.object({
  name: z.string(),
  data_type: z.enum(ScoreType),
  mean: z.number().nullable(),
  count: z.number(),
  categories: z.array(z.string()).default([]),
});
export type ScoreSummary = z.infer<typeof ScoreSummarySchema>;

/** A trace's id as the path and the lanes take it: OpenTelemetry's, 32
 *  lowercase hex digits, never all zeros. */
export const TraceIdSchema = z.string().regex(/^(?!0{32}$)[0-9a-f]{32}$/);

// One trace of the list: its spans summed by the server, which reads the
// project's spans and no other's. `user_id` is as stored, hashed, and
// `ended` says whether the agent's own span has
// arrived; until it has, `status` is read off the spans that have.
export const TraceSchema = z.object({
  trace_id: z.string(),
  agent: z.string(),
  user_id: z.string(),
  session_id: z.string(),
  ended: z.boolean(),
  status: z.enum(SpanStatus),
  /** The class of the error the agent's own span ended on, never its
   *  message; empty otherwise. */
  error_type: z.string(),
  started_at: z.string(),
  ended_at: z.string(),
  duration_ms: z.number(),
  /** Its first model call's time to first token that reported one, in milliseconds. */
  time_to_first_token_ms: z.number().nullable(),
  spans: z.number(),
  steps: z.number(),
  model_calls: z.number(),
  tool_calls: z.number(),
  retrievals: z.number(),
  errors: z.number(),
  input_tokens: z.number(),
  output_tokens: z.number(),
  cache_read_input_tokens: z.number(),
  cache_write_input_tokens: z.number(),
  reasoning_output_tokens: z.number(),
  /** What its model calls cost in US dollars, reported or priced: its own,
   *  never scaled by a sample rate. */
  spend: z.number(),
  models: z.array(z.string()),
  tools: z.array(z.string()),
  /** The deployments its spans were sent from, each once: one, unless the
   *  trace crossed services deployed apart. */
  environments: z.array(z.string()),
  /** The releases its spans were sent from, each once. */
  releases: z.array(z.string()),
  /** Its tags, each `key:value` once, in order. */
  tags: z.array(z.string()),
  /** Its own scores by name, where the list read them. */
  scores: z.array(ScoreSummarySchema).default([]),
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
  reasoning_output_tokens: z.number(),
  /** What it cost in US dollars, as reported; null where it reported none. */
  cost: z.number().nullable(),
  /** Seconds to its answer's first token; null where it reported none. */
  time_to_first_token: z.number().nullable(),
  /** Output tokens/s after the first token; null without valid measurements. */
  output_speed: z.number().positive().nullable(),
  fault: z.enum(SpanFault).nullable(),
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

/** What a trace has spent, as an alert on its cost reads it: each call's
 *  cost as reported, else its tokens at Telmoni's price, and the models
 *  priced by neither, which it leaves out. */
export const TraceSpendSchema = z.object({
  spent: z.number(),
  unpriced: z.array(z.object({ provider: z.string(), model: z.string() })),
});
export type TraceSpend = z.infer<typeof TraceSpendSchema>;

/** One span's content: each field its project kept, by name, as it arrived. */
export const SpanFieldsSchema = z.object({
  trace_id: z.string(),
  span_id: z.string(),
  fields: z.record(z.string(), z.string()),
});
export type SpanFields = z.infer<typeof SpanFieldsSchema>;

export const TraceDetailSchema = z.object({
  trace_id: z.string(),
  spans: z.array(TraceSpanSchema),
  truncated: z.boolean(),
  /** None where the server could not read it. */
  spend: TraceSpendSchema.nullable(),
  /** Each span's content, its read filed in the audit log: empty where the
   *  trace kept none, null where it was not asked for or could not be opened. */
  content: z.array(SpanFieldsSchema).nullable(),
});
export type TraceDetail = z.infer<typeof TraceDetailSchema>;

/** A trace as the list's peek reads it (`/api/v1/traces/[trace]`): its
 *  spans, the incidents whose notices showed it, as its line names them, and
 *  its scores, `null` where they could not be read. */
export const PeekTraceSchema = TraceDetailSchema.extend({
  incidents: z
    // An alert's incident names no agent, a monitor's no alert.
    .array(z.object({ id: z.string(), rule_name: z.string().nullable(), agent: z.string().nullable() }))
    .default([]),
  scores: z.array(ScoreSchema).nullable().default(null),
});
