import { cache } from "react";
import { z } from "zod";

import { env } from "@/lib/env";
import { queryString } from "@/lib/list-query";
import { ChartMeasure, MetricsGroup, SortDirection, SpanFault, TraceSort } from "@/lib/types/enums";

import {
  FILTER_KEYS,
  ScoreSummarySchema,
  TagSortSchema,
  TraceDetailSchema,
  TraceSchema,
  filtersIn,
  type FilterKey,
  type ListFilters,
  type TagSort,
  type TraceDetail,
} from "@/lib/types/traces";

import { identityContext, projectHeaders } from "./identity-context";
import { type Read, readBytes, readJson } from "./read";

export {
  FILTER_KEYS,
  ListFiltersSchema,
  TraceDetailSchema,
  TraceSchema,
  filtersIn,
  type FilterKey,
  type ListFilters,
  type SpanFields,
  type TraceSpend,
  type Trace,
  type TraceDetail,
  type TraceSpan,
} from "@/lib/types/traces";

// The list's sort as it answers and keeps one: a word of its columns', or a
// tag's.
const TraceSortSchema = z.union([z.enum(TraceSort), TagSortSchema]);

// A page of the list, and how many traces the filters admit on every page.
export const TraceListSchema = z
  .object({
    traces: z.array(TraceSchema),
    days: z.number(),
    limit: z.number(),
    page: z.number(),
    total: z.number(),
    more: z.boolean(),
    sort: TraceSortSchema,
    dir: z.enum(SortDirection),
    /** Each trace's own scores by name, keyed by its id: put on it. */
    scores: z.record(z.string(), z.array(ScoreSummarySchema)).default({}),
  })
  .transform(({ scores, ...list }) => ({
    ...list,
    traces: list.traces.map((trace) => ({ ...trace, scores: scores[trace.trace_id] ?? [] })),
  }));
export type TraceList = z.infer<typeof TraceListSchema>;

// One filter's values among the window's traces, each with how many of the
// list's rows carry it under every other filter: the ticked first.
const FacetSchema = z.object({
  values: z.array(z.object({ value: z.string(), count: z.number() })),
  more: z.boolean(),
});
export type Facet = z.infer<typeof FacetSchema>;

const facetShape = Object.fromEntries(FILTER_KEYS.map((key) => [key, FacetSchema])) as Record<
  FilterKey,
  typeof FacetSchema
>;

export const FacetsSchema = z.object({ days: z.number(), ...facetShape });
export type Facets = z.infer<typeof FacetsSchema>;

// The window in bars of equal width, each starting on the reader's clock:
// the traces that started in it by how they ended, their tokens, and the
// median and 95th percentile of their durations and of their times to first
// token, none where no trace started or reported one.
const HistogramBarSchema = z.object({
  at: z.string(),
  ok: z.number(),
  error: z.number(),
  cancelled: z.number(),
  faults: z.partialRecord(z.enum(SpanFault), z.number().int().nonnegative()),
  tokens: z.number(),
  /** What its traces' model calls cost, each trace its own. */
  spend: z.number(),
  duration_p50_ms: z.number().nullable(),
  duration_p95_ms: z.number().nullable(),
  time_to_first_token_p50_ms: z.number().nullable(),
  time_to_first_token_p95_ms: z.number().nullable(),
  /** The mean of the narrowing score's scores given in it; none without one. */
  score_mean: z.number().nullable(),
});

export const TraceHistogramSchema = z.object({
  days: z.number(),
  bucket_seconds: z.number(),
  /** The score the list is narrowed to, whose mean each bar holds. */
  score: z.string().nullable(),
  utc_offset: z.number(),
  bars: z.array(HistogramBarSchema),
});
export type TraceHistogram = z.infer<typeof TraceHistogramSchema>;

/** A dashboard's charts: what each draws and, for spend, what splits it. */
export const DashboardQuerySchema = z.object({
  charts: z
    .array(
      z.object({
        measure: z.enum(ChartMeasure),
        group: z.enum(MetricsGroup).optional(),
        tag: z.string().optional(),
      }),
    )
    .max(64),
});

// One of the project's saved dashboards: its charts, under a name.
export const DashboardViewSchema = z.object({
  id: z.string(),
  name: z.string(),
  query: DashboardQuerySchema,
  created_at: z.string(),
  updated_at: z.string(),
});

export const DashboardViewListSchema = z.object({ views: z.array(DashboardViewSchema) });

/** What narrows and orders a list, and which page of it shows, as the URL
 *  carries it, `S` the sorts it takes. The server checks every value; the
 *  page drops what it cannot read before asking (`lib/traces.ts`). */
export type ListQuery<S extends string> = ListFilters & {
  /** A score the traces carry, by its name, and its value's least and most. */
  score?: string;
  score_min?: number;
  score_max?: number;
  sort?: S;
  dir?: SortDirection;
  days?: number;
  limit?: number;
  page?: number;
};

export type TraceQuery = ListQuery<TraceSort | TagSort>;

/** The filters and the window alone: what the facets and the histogram take,
 *  each lane refusing a parameter it has no use for. */
export function filtersOf(
  query: ListFilters & { days?: number; score?: string; score_min?: number; score_max?: number },
) {
  return {
    ...filtersIn(query),
    score: query.score,
    score_min: query.score_min,
    score_max: query.score_max,
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
  | { kind: "ok"; facets: Facets }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type HistogramAccess =
  | { kind: "ok"; histogram: TraceHistogram }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type DownloadAccess =
  | { kind: "ok"; body: ArrayBuffer; contentType: string }
  | { kind: "refused" }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type TraceAccess =
  | { kind: "ok"; trace: TraceDetail }
  /** No trace with that id in this project. */
  | { kind: "missing" }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** How long a download may take: ten thousand rows summed and written, which
 *  a page's ordinary deadline would cut short. */
const DOWNLOAD_TIMEOUT_MS = 30_000;

/** How one of telemetry's lanes is asked for the project: its URL and the
 *  request's headers, or none where nobody is signed in. */
async function laneRequest(projectId: string, path: string): Promise<[string, RequestInit] | null> {
  const ctx = projectId ? await identityContext() : null;
  if (!ctx) return null;
  return [
    `${env.SERVER_URL}/internal/v1/telemetry${path}`,
    { headers: projectHeaders(ctx, projectId), cache: "no-store" },
  ];
}

/** One of telemetry's lanes as the project reads it, its answer held to
 *  `schema`. */
export async function readLane<T, E extends string = never>(
  fetcher: string,
  projectId: string,
  path: string,
  schema: z.ZodType<T>,
  statuses?: Readonly<Partial<Record<number, E>>>,
): Promise<Read<T, E>> {
  const request = await laneRequest(projectId, path);
  if (!request) return { kind: "unavailable" };
  return readJson(fetcher, ...request, schema, statuses);
}

/** A file the project keeps, as its lane serves it: its bytes and type. */
export type MediaAccess =
  | { kind: "ok"; body: ArrayBuffer; contentType: string }
  | { kind: "missing" }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** A file the project keeps, by the hash of its bytes; its read is filed by
 *  the server before it answers. */
export async function fetchMedia(projectId: string, sha256: string): Promise<MediaAccess> {
  const request = await laneRequest(projectId, `/media/${sha256}`);
  if (!request) return { kind: "unavailable" };
  return readBytes("fetchMedia", ...request, MISSING);
}

/** A path the server cannot read as a trace, or one it has none of: the page
 *  says it is not there. */
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

/** A list as a file from one of telemetry's download lanes, `path` with its
 *  query: a 400 is a query the server refused, which the route says so
 *  about. */
export async function readDownload(
  fetcher: string,
  projectId: string,
  path: string,
): Promise<DownloadAccess> {
  const request = await laneRequest(projectId, path);
  if (!request) return { kind: "unavailable" };
  return readBytes(fetcher, ...request, { 400: "refused" }, DOWNLOAD_TIMEOUT_MS);
}

/** The list as a file: every trace the filters admit, sorted as the page is,
 *  up to the server's cap. */
export async function traceDownload(
  projectId: string,
  query: TraceQuery,
  format: DownloadFormat,
): Promise<DownloadAccess> {
  const lane = queryString({
    ...filtersOf(query),
    sort: query.sort,
    dir: query.dir,
    format,
  });
  return readDownload("traceDownload", projectId, `/traces/export${lane}`);
}

/** One trace's spans, by its id, and its content where `content` asks, which
 *  the server files as the reader's read: every caller says which. An id the
 *  path cannot be — not 32 hex digits — is read as missing, as the server
 *  would answer a trace that is not the project's. */
export const fetchTrace = cache(
  async (projectId: string, traceId: string, content: boolean): Promise<TraceAccess> => {
    const read = await readLane(
      "fetchTrace",
      projectId,
      `/traces/${encodeURIComponent(traceId)}${content ? "?content=true" : ""}`,
      TraceDetailSchema,
      MISSING,
    );
    return read.kind === "ok" ? { kind: "ok", trace: read.value } : read;
  },
);
