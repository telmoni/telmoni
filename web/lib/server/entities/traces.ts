import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { queryString } from "@/lib/list-query";
import { logger } from "@/lib/logger";
import {
  LayoutDepth,
  LayoutOpen,
  SortDirection,
  SpanKind,
  SpanStatus,
  TraceColumn,
  TraceSort,
  ViewKind,
} from "@/lib/types/enums";

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

// A page of the list, and how many traces the filters admit on every page.
const TraceListSchema = z.object({
  traces: z.array(TraceSchema),
  days: z.number(),
  limit: z.number(),
  page: z.number(),
  total: z.number(),
  more: z.boolean(),
  sort: z.enum(TraceSort),
  dir: z.enum(SortDirection),
});
export type TraceList = z.infer<typeof TraceListSchema>;

// One filter's values among the window's runs, each counted under every
// other filter: the ticked ones first, then the most runs.
const FacetSchema = z.object({
  values: z.array(z.object({ value: z.string(), runs: z.number() })),
  more: z.boolean(),
});
export type Facet = z.infer<typeof FacetSchema>;

const TraceFacetsSchema = z.object({
  days: z.number(),
  agent: FacetSchema,
  user: FacetSchema,
  session: FacetSchema,
  status: FacetSchema,
  model: FacetSchema,
  tool: FacetSchema,
  environment: FacetSchema,
  release: FacetSchema,
  /** Each tag as `key:value`. */
  tag: FacetSchema,
  prompt: FacetSchema,
});
export type TraceFacets = z.infer<typeof TraceFacetsSchema>;

// The window in bars of equal width, each starting on the reader's clock:
// the runs that started in it by how they ended, their tokens, and the
// median and 95th percentile of their durations, none where no run started.
const HistogramBarSchema = z.object({
  at: z.string(),
  ok: z.number(),
  error: z.number(),
  cancelled: z.number(),
  tokens: z.number(),
  duration_p50_ms: z.number().nullable(),
  duration_p95_ms: z.number().nullable(),
});

const TraceHistogramSchema = z.object({
  days: z.number(),
  bucket_seconds: z.number(),
  utc_offset: z.number(),
  bars: z.array(HistogramBarSchema),
});
export type TraceHistogram = z.infer<typeof TraceHistogramSchema>;

// A saved view's query: the list's, each filter's values a list, and no
// key the list does not take, as the server refuses one.
export const ViewQuerySchema = z.strictObject({
  agent: z.array(z.string()).optional(),
  user: z.array(z.string()).optional(),
  session: z.array(z.string()).optional(),
  status: z.array(z.enum(SpanStatus)).optional(),
  model: z.array(z.string()).optional(),
  tool: z.array(z.string()).optional(),
  environment: z.array(z.string()).optional(),
  release: z.array(z.string()).optional(),
  tag: z.array(z.string()).optional(),
  prompt: z.array(z.string()).optional(),
  sort: z.enum(TraceSort).optional(),
  dir: z.enum(SortDirection).optional(),
  days: z.number().optional(),
  limit: z.number().optional(),
});
export type ViewQuery = z.infer<typeof ViewQuerySchema>;

// The calls a layout shows: by kind, by how they ended and by name, any
// value of each admitting a call, and a list left out every call.
export const LayoutFilterSchema = z.strictObject({
  kind: z.array(z.enum(SpanKind)).optional(),
  status: z.array(z.enum(SpanStatus)).optional(),
  name: z.array(z.string()).optional(),
});

// A layout of a session's page and a trace's, as a saved layout's `query`
// holds it, its columns beside it (`lib/layout.ts` reads both as one).
export const LayoutQuerySchema = z.strictObject({
  depth: z.enum(LayoutDepth),
  open: z.enum(LayoutOpen),
  timeline: z.boolean(),
  pinned: z.array(z.string()).optional(),
  filter: LayoutFilterSchema,
  messages: z.boolean(),
  scores: z.boolean(),
});
export type LayoutQuery = z.infer<typeof LayoutQuerySchema>;

const SavedViewSchema = z.object({
  id: z.string(),
  name: z.string(),
  columns: z.array(z.enum(TraceColumn)),
  created_at: z.string(),
  updated_at: z.string(),
});

// One of the project's saved views of the list: its query and the columns it
// showed, under a name.
export const TraceViewSchema = SavedViewSchema.extend({
  kind: z.literal(ViewKind.List),
  query: ViewQuerySchema,
});
export type TraceView = z.infer<typeof TraceViewSchema>;

// One of the project's saved layouts: a layout's options and the columns a
// trace shows, under a name.
export const TraceLayoutSchema = SavedViewSchema.extend({
  kind: z.literal(ViewKind.Layout),
  query: LayoutQuerySchema,
});
export type TraceLayout = z.infer<typeof TraceLayoutSchema>;

/** A saved view of either kind, as a save answers it. */
export const SavedViewOfAnyKindSchema = z.discriminatedUnion("kind", [
  TraceViewSchema,
  TraceLayoutSchema,
]);

const TraceViewListSchema = z.object({ views: z.array(TraceViewSchema) });
const TraceLayoutListSchema = z.object({ views: z.array(TraceLayoutSchema) });

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
  /** The deployment its resource named, and its release; empty for none. */
  environment: z.string(),
  release: z.string(),
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

/** What narrows and orders the list, as the URL carries it: a filter is the
 *  values any of which admits a run, and a key left out narrows nothing.
 *  The server checks every value; the page drops what it cannot read before
 *  asking (`lib/traces.ts`). */
export type TraceQuery = {
  agent?: string[];
  user?: string[];
  session?: string[];
  status?: SpanStatus[];
  model?: string[];
  tool?: string[];
  environment?: string[];
  release?: string[];
  /** Each `key:value`. */
  tag?: string[];
  prompt?: string[];
  sort?: TraceSort;
  dir?: SortDirection;
  days?: number;
  limit?: number;
  page?: number;
};

/** The filters and the window alone: what the facets and the histogram take,
 *  each lane refusing a parameter it has no use for. */
function filtersOf(query: TraceQuery) {
  return {
    agent: query.agent,
    user: query.user,
    session: query.session,
    status: query.status,
    model: query.model,
    tool: query.tool,
    environment: query.environment,
    release: query.release,
    tag: query.tag,
    prompt: query.prompt,
    days: query.days,
  };
}

/** The two shapes a download comes in. */
export type DownloadFormat = "csv" | "json";

export type TracesAccess =
  | { kind: "ok"; list: TraceList }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type FacetsAccess =
  | { kind: "ok"; facets: TraceFacets }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type HistogramAccess =
  | { kind: "ok"; histogram: TraceHistogram }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type ViewsAccess =
  | { kind: "ok"; views: TraceView[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type LayoutsAccess =
  | { kind: "ok"; layouts: TraceLayout[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type DownloadAccess =
  | { kind: "ok"; body: ArrayBuffer; contentType: string }
  | { kind: "refused" }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type TraceAccess =
  | { kind: "ok"; trace: TraceDetail }
  /** No run with that id in this project. */
  | { kind: "missing" }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** How long a download may take: ten thousand rows summed and written, which
 *  a page's ordinary deadline would cut short. */
const DOWNLOAD_TIMEOUT_MS = 30_000;

function fetchFailed(fetcher: string, err: unknown): void {
  logger.warn(
    { fetcher, error: err instanceof Error ? err.message : String(err) },
    "entities: fetch failed",
  );
}

/** One of telemetry's trace lanes as the project reads it, its answer held to
 *  `schema`: forbidden on a 403, unavailable on anything else it cannot read. */
async function readLane<T>(
  fetcher: string,
  projectId: string,
  path: string,
  schema: z.ZodType<T>,
): Promise<{ kind: "ok"; value: T } | { kind: "forbidden" } | { kind: "unavailable" }> {
  if (!projectId) return { kind: "unavailable" };
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  try {
    const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/telemetry${path}`, {
      headers: projectHeaders(ctx, projectId),
      cache: "no-store",
    });
    if (res.status === 403) return { kind: "forbidden" };
    if (!res.ok) {
      logger.warn({ fetcher, status: res.status }, "entities: upstream refused");
      return { kind: "unavailable" };
    }
    const parsed = schema.safeParse(await res.json());
    if (!parsed.success) {
      logger.warn({ fetcher, issues: parsed.error.issues }, "entities: upstream shape mismatch");
      return { kind: "unavailable" };
    }
    return { kind: "ok", value: parsed.data };
  } catch (err) {
    fetchFailed(fetcher, err);
    return { kind: "unavailable" };
  }
}

/** A page of the project's traces in the window, as the server lists them. */
export const fetchTraces = cache(
  async (projectId: string, query: TraceQuery): Promise<TracesAccess> => {
    const read = await readLane("fetchTraces", projectId, `/traces${queryString(query)}`, TraceListSchema);
    return read.kind === "ok" ? { kind: "ok", list: read.value } : read;
  },
);

/** Each filter's values among the project's traces, counted under every
 *  other filter. */
export const fetchTraceFacets = cache(
  async (projectId: string, query: TraceQuery): Promise<FacetsAccess> => {
    const read = await readLane(
      "fetchTraceFacets",
      projectId,
      `/traces/facets${queryString(filtersOf(query))}`,
      TraceFacetsSchema,
    );
    return read.kind === "ok" ? { kind: "ok", facets: read.value } : read;
  },
);

/** The project's traces the filters admit, by the bar they started in, the
 *  bars starting on the reader's clock, `utcOffset` minutes east of UTC. */
export const fetchTraceHistogram = cache(
  async (projectId: string, query: TraceQuery, utcOffset: number): Promise<HistogramAccess> => {
    const read = await readLane(
      "fetchTraceHistogram",
      projectId,
      `/traces/histogram${queryString({ ...filtersOf(query), utc_offset: utcOffset })}`,
      TraceHistogramSchema,
    );
    return read.kind === "ok" ? { kind: "ok", histogram: read.value } : read;
  },
);

/** The project's saved views of the list, by name. */
export const fetchTraceViews = cache(async (projectId: string): Promise<ViewsAccess> => {
  const read = await readLane(
    "fetchTraceViews",
    projectId,
    `/views?kind=${ViewKind.List}`,
    TraceViewListSchema,
  );
  return read.kind === "ok" ? { kind: "ok", views: read.value.views } : read;
});

/** The project's saved layouts of a session's page and a trace's, by name. */
export const fetchTraceLayouts = cache(async (projectId: string): Promise<LayoutsAccess> => {
  const read = await readLane(
    "fetchTraceLayouts",
    projectId,
    `/views?kind=${ViewKind.Layout}`,
    TraceLayoutListSchema,
  );
  return read.kind === "ok" ? { kind: "ok", layouts: read.value.views } : read;
});

/** The list as a file: every trace the filters admit, sorted as the page is,
 *  up to the server's cap — or the runs `runs` names alone. A 400 is a query
 *  the server refused, which the route says so about. */
export async function traceDownload(
  projectId: string,
  query: TraceQuery,
  format: DownloadFormat,
  runs: string[],
): Promise<DownloadAccess> {
  if (!projectId) return { kind: "unavailable" };
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  const lane = queryString({
    ...filtersOf(query),
    sort: query.sort,
    dir: query.dir,
    format,
    run: runs,
  });
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/telemetry/traces/export${lane}`,
      { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      DOWNLOAD_TIMEOUT_MS,
    );
    if (res.status === 403) return { kind: "forbidden" };
    if (res.status === 400) return { kind: "refused" };
    if (!res.ok) {
      logger.warn({ fetcher: "traceDownload", status: res.status }, "entities: upstream refused");
      return { kind: "unavailable" };
    }
    return {
      kind: "ok",
      body: await res.arrayBuffer(),
      contentType: res.headers.get("content-type") ?? "application/octet-stream",
    };
  } catch (err) {
    fetchFailed("traceDownload", err);
    return { kind: "unavailable" };
  }
}

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
