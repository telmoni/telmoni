import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { queryString } from "@/lib/list-query";
import { logger } from "@/lib/logger";
import {
  LayoutDepth,
  LayoutOpen,
  SessionColumn,
  SessionSort,
  SortDirection,
  SpanKind,
  SpanStatus,
  TraceColumn,
  TraceSort,
  UserColumn,
  UserSort,
  ViewKind,
} from "@/lib/types/enums";

import {
  FILTER_KEYS,
  ListFiltersSchema,
  TraceDetailSchema,
  TraceSchema,
  filtersIn,
  type FilterKey,
  type ListFilters,
  type TraceDetail,
} from "@/lib/types/traces";

import { identityContext, projectHeaders } from "./identity-context";
import { type Read, readFailed, readJson } from "./read";

export {
  FILTER_KEYS,
  ListFiltersSchema,
  TraceDetailSchema,
  TraceSchema,
  filtersIn,
  type FilterKey,
  type ListFilters,
  type RunSpend,
  type Trace,
  type TraceDetail,
  type TraceSpan,
} from "@/lib/types/traces";

// A page of the list, and how many traces the filters admit on every page.
export const TraceListSchema = z.object({
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

export const TraceHistogramSchema = z.object({
  days: z.number(),
  bucket_seconds: z.number(),
  utc_offset: z.number(),
  bars: z.array(HistogramBarSchema),
});
export type TraceHistogram = z.infer<typeof TraceHistogramSchema>;

// A saved view's query: its list's, each filter's values a list, its sort
// one of that list's, and no key the list does not take, as the server
// refuses one.
function viewQuerySchema<S extends Record<string, string>>(sorts: S) {
  return z.strictObject({
    ...ListFiltersSchema.shape,
    sort: z.enum(sorts).optional(),
    dir: z.enum(SortDirection).optional(),
    days: z.number().optional(),
    limit: z.number().optional(),
  });
}
export const ViewQuerySchema = viewQuerySchema(TraceSort);
export const SessionViewQuerySchema = viewQuerySchema(SessionSort);
export const UserViewQuerySchema = viewQuerySchema(UserSort);
/** A saved view's query, its sort among `S`. */
export type ListViewQuery<S extends string> = ListFilters & {
  sort?: S;
  dir?: SortDirection;
  days?: number;
  limit?: number;
};

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

// One of the project's saved views of a list: its query and the columns it
// showed, under a name, each by that list's words.
export const TraceViewSchema = SavedViewSchema.extend({
  kind: z.literal(ViewKind.Traces),
  query: ViewQuerySchema,
});
export type TraceView = z.infer<typeof TraceViewSchema>;

export const SessionViewSchema = SavedViewSchema.extend({
  kind: z.literal(ViewKind.Sessions),
  query: SessionViewQuerySchema,
  columns: z.array(z.enum(SessionColumn)),
});
export type SessionView = z.infer<typeof SessionViewSchema>;

export const UserViewSchema = SavedViewSchema.extend({
  kind: z.literal(ViewKind.Users),
  query: UserViewQuerySchema,
  columns: z.array(z.enum(UserColumn)),
});
export type UserView = z.infer<typeof UserViewSchema>;

/** A saved view of any of the three lists. */
export type ListView = TraceView | SessionView | UserView;

// One of the project's saved layouts: a layout's options and the columns a
// trace shows, under a name.
export const TraceLayoutSchema = SavedViewSchema.extend({
  kind: z.literal(ViewKind.Layout),
  query: LayoutQuerySchema,
});
export type TraceLayout = z.infer<typeof TraceLayoutSchema>;

/** A saved view of any kind, as a save answers it. */
export const SavedViewOfAnyKindSchema = z.discriminatedUnion("kind", [
  TraceViewSchema,
  SessionViewSchema,
  UserViewSchema,
  TraceLayoutSchema,
]);

export const TraceViewListSchema = z.object({ views: z.array(TraceViewSchema) });
export const SessionViewListSchema = z.object({ views: z.array(SessionViewSchema) });
export const UserViewListSchema = z.object({ views: z.array(UserViewSchema) });
export const TraceLayoutListSchema = z.object({ views: z.array(TraceLayoutSchema) });

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

export type ViewsAccess<V extends ListView = TraceView> =
  | { kind: "ok"; views: V[] }
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

/** The project's saved views of the trace list, by name. */
export const fetchTraceViews = cache(async (projectId: string): Promise<ViewsAccess> => {
  const read = await readLane(
    "fetchTraceViews",
    projectId,
    `/views?kind=${ViewKind.Traces}`,
    TraceViewListSchema,
  );
  return read.kind === "ok" ? { kind: "ok", views: read.value.views } : read;
});

/** The project's saved views of the sessions list, by name. */
export const fetchSessionViews = cache(
  async (projectId: string): Promise<ViewsAccess<SessionView>> => {
    const read = await readLane(
      "fetchSessionViews",
      projectId,
      `/views?kind=${ViewKind.Sessions}`,
      SessionViewListSchema,
    );
    return read.kind === "ok" ? { kind: "ok", views: read.value.views } : read;
  },
);

/** The project's saved views of the users list, by name. */
export const fetchUserViews = cache(async (projectId: string): Promise<ViewsAccess<UserView>> => {
  const read = await readLane(
    "fetchUserViews",
    projectId,
    `/views?kind=${ViewKind.Users}`,
    UserViewListSchema,
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
