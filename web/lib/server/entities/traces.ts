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
import { type Read, readFailed, readJson } from "./read";

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

// One filter's values among the window's runs, each with how many of the
// list's rows carry it under every other filter: the ticked first.
const FacetSchema = z.object({
  values: z.array(z.object({ value: z.string(), count: z.number() })),
  more: z.boolean(),
});
export type Facet = z.infer<typeof FacetSchema>;

const Values = z.array(z.string()).optional();

/** What narrows a list as the URL carries it, in the sidebar's order: a
 *  filter is the values any of which admits a run, a key left out narrowing
 *  nothing; a session or a user is admitted by a run of theirs. The one list
 *  of the keys: the facets, a saved view and every lane's query take them
 *  from here. */
export const ListFiltersSchema = z.object({
  agent: Values,
  status: z.array(z.enum(SpanStatus)).optional(),
  model: Values,
  tool: Values,
  user: Values,
  session: Values,
  environment: Values,
  release: Values,
  /** Each `key:value`. */
  tag: Values,
  prompt: Values,
});
export type ListFilters = z.infer<typeof ListFiltersSchema>;
export type FilterKey = keyof ListFilters;
/** The filters' keys, in the sidebar's order. */
export const FILTER_KEYS = ListFiltersSchema.keyof().options;

/** `query`'s filters alone, every key present. */
export function filtersIn(query: ListFilters): ListFilters {
  return Object.fromEntries(FILTER_KEYS.map((key) => [key, query[key]])) as ListFilters;
}

const facetShape = Object.fromEntries(FILTER_KEYS.map((key) => [key, FacetSchema])) as Record<
  FilterKey,
  typeof FacetSchema
>;

export const FacetsSchema = z.object({ days: z.number(), ...facetShape });
export type Facets = z.infer<typeof FacetsSchema>;

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
  ...ListFiltersSchema.shape,
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
const RunSpendSchema = z.object({
  spent: z.number(),
  unpriced: z.array(z.object({ provider: z.string(), model: z.string() })),
});
export type RunSpend = z.infer<typeof RunSpendSchema>;

const TraceDetailSchema = z.object({
  run_id: z.string(),
  spans: z.array(TraceSpanSchema),
  truncated: z.boolean(),
  /** None where the server could not read it. */
  spend: RunSpendSchema.nullable(),
});
export type TraceDetail = z.infer<typeof TraceDetailSchema>;

/** What narrows and orders a list, and which page of it shows, as the URL
 *  carries it, `S` its columns' sorts. The server checks every value; the
 *  page drops what it cannot read before asking (`lib/traces.ts`). */
export type ListQuery<S extends string> = ListFilters & {
  sort?: S;
  dir?: SortDirection;
  days?: number;
  limit?: number;
  page?: number;
};

export type TraceQuery = ListQuery<TraceSort>;

/** The filters and the window alone: what the facets and the histogram take,
 *  each lane refusing a parameter it has no use for. */
export function filtersOf(query: ListFilters & { days?: number }) {
  return { ...filtersIn(query), days: query.days };
}

/** The two shapes a download comes in. */
export type DownloadFormat = "csv" | "json";

export type TracesAccess =
  | { kind: "ok"; list: TraceList }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type FacetsAccess =
  | { kind: "ok"; facets: Facets }
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

/** One of telemetry's lanes as the project reads it, its answer held to
 *  `schema`. */
export async function readLane<T, E extends string = never>(
  fetcher: string,
  projectId: string,
  path: string,
  schema: z.ZodType<T>,
  statuses?: Readonly<Partial<Record<number, E>>>,
): Promise<Read<T, E>> {
  const ctx = projectId ? await identityContext() : null;
  if (!ctx) return { kind: "unavailable" };
  return readJson(
    fetcher,
    `${env.SERVER_URL}/internal/v1/telemetry${path}`,
    { headers: projectHeaders(ctx, projectId), cache: "no-store" },
    schema,
    statuses,
  );
}

/** A path the server cannot read as a run or a session, or one it has none
 *  of: the page says it is not there. */
export const MISSING = { 400: "missing", 404: "missing" } as const;

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
      FacetsSchema,
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

/** A list as a file from one of telemetry's download lanes, `path` with its
 *  query: a 400 is a query the server refused, which the route says so
 *  about. */
export async function readDownload(
  fetcher: string,
  projectId: string,
  path: string,
): Promise<DownloadAccess> {
  if (!projectId) return { kind: "unavailable" };
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/v1/telemetry${path}`,
      { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      DOWNLOAD_TIMEOUT_MS,
    );
    if (res.status === 403) return { kind: "forbidden" };
    if (res.status === 400) return { kind: "refused" };
    if (!res.ok) {
      logger.warn({ fetcher, status: res.status }, "entities: upstream refused");
      return { kind: "unavailable" };
    }
    return {
      kind: "ok",
      body: await res.arrayBuffer(),
      contentType: res.headers.get("content-type") ?? "application/octet-stream",
    };
  } catch (err) {
    readFailed(fetcher, err);
    return { kind: "unavailable" };
  }
}

/** The list as a file: every trace the filters admit, sorted as the page is,
 *  up to the server's cap — or the runs `runs` names alone. */
export async function traceDownload(
  projectId: string,
  query: TraceQuery,
  format: DownloadFormat,
  runs: string[],
): Promise<DownloadAccess> {
  const lane = queryString({
    ...filtersOf(query),
    sort: query.sort,
    dir: query.dir,
    format,
    run: runs,
  });
  return readDownload("traceDownload", projectId, `/traces/export${lane}`);
}

/** One trace's spans, by the run's id. A run id the path cannot be — not a
 *  UUID — is read as missing, as the server would answer a run that is not
 *  the project's. */
export const fetchTrace = cache(
  async (projectId: string, runId: string): Promise<TraceAccess> => {
    const read = await readLane(
      "fetchTrace",
      projectId,
      `/traces/${encodeURIComponent(runId)}`,
      TraceDetailSchema,
      MISSING,
    );
    return read.kind === "ok" ? { kind: "ok", trace: read.value } : read;
  },
);
