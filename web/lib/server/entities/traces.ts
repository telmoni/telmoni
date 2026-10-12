import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { queryString } from "@/lib/list-query";
import { logger } from "@/lib/logger";
import { SortDirection, SpanKind, SpanStatus, TraceSort } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";

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
});
export type Trace = z.infer<typeof TraceSchema>;

const TraceListSchema = z.object({
  traces: z.array(TraceSchema),
  days: z.number(),
  limit: z.number(),
  more: z.boolean(),
  sort: z.enum(TraceSort),
  dir: z.enum(SortDirection),
});
export type TraceList = z.infer<typeof TraceListSchema>;

// One span of a trace, as the server answers it: the row with its maps as
// objects and its times as text.
const TraceSpanSchema = z.object({
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
  user_id: z.string(),
  session_id: z.string(),
  tags: z.record(z.string(), z.string()),
  bound_stall_after_s: z.number().nullable(),
  bound_max_steps: z.number().nullable(),
  bound_max_cost_usd: z.number().nullable(),
  sample_rate: z.number().nullable(),
  attributes: z.record(z.string(), z.string()),
  units: z.number(),
  received_at: z.string(),
});
export type TraceSpan = z.infer<typeof TraceSpanSchema>;

const TraceDetailSchema = z.object({
  run_id: z.string(),
  spans: z.array(TraceSpanSchema),
  truncated: z.boolean(),
});
export type TraceDetail = z.infer<typeof TraceDetailSchema>;

/** What narrows and orders the list, as the URL carries it: a key left out
 *  narrows nothing. The server checks every value; the page drops what it
 *  cannot read before asking (`lib/traces.ts`). */
export type TraceQuery = {
  agent?: string;
  user?: string;
  session?: string;
  status?: SpanStatus;
  model?: string;
  tool?: string;
  sort?: TraceSort;
  dir?: SortDirection;
  days?: number;
  limit?: number;
};

export type TracesAccess =
  | { kind: "ok"; list: TraceList }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type TraceAccess =
  | { kind: "ok"; trace: TraceDetail }
  /** No run with that id in this project. */
  | { kind: "missing" }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

function fetchFailed(fetcher: string, err: unknown): void {
  logger.warn(
    { fetcher, error: err instanceof Error ? err.message : String(err) },
    "entities: fetch failed",
  );
}

/** The project's traces in the window, as the server lists them. */
export const fetchTraces = cache(
  async (projectId: string, query: TraceQuery): Promise<TracesAccess> => {
    if (!projectId) return { kind: "unavailable" };
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/telemetry/traces${queryString(query)}`,
        { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      );
      if (res.status === 403) return { kind: "forbidden" };
      if (!res.ok) {
        logger.warn({ fetcher: "fetchTraces", status: res.status }, "entities: upstream refused");
        return { kind: "unavailable" };
      }
      const parsed = TraceListSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchTraces", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return { kind: "unavailable" };
      }
      return { kind: "ok", list: parsed.data };
    } catch (err) {
      fetchFailed("fetchTraces", err);
      return { kind: "unavailable" };
    }
  },
);

/** One trace's spans, by the run's id. A run id the path cannot be — not a
 *  UUID — is read as missing, as the server would answer a run that is not
 *  the project's. */
export const fetchTrace = cache(
  async (projectId: string, runId: string): Promise<TraceAccess> => {
    if (!projectId) return { kind: "unavailable" };
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/telemetry/traces/${encodeURIComponent(runId)}`,
        { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      );
      if (res.status === 403) return { kind: "forbidden" };
      if (res.status === 404 || res.status === 400) return { kind: "missing" };
      if (!res.ok) {
        logger.warn({ fetcher: "fetchTrace", status: res.status }, "entities: upstream refused");
        return { kind: "unavailable" };
      }
      const parsed = TraceDetailSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchTrace", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return { kind: "unavailable" };
      }
      return { kind: "ok", trace: parsed.data };
    } catch (err) {
      fetchFailed("fetchTrace", err);
      return { kind: "unavailable" };
    }
  },
);
