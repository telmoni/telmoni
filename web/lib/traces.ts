import type {
  DownloadFormat,
  ListFilters,
  ListQuery,
  TraceQuery,
  TraceSpan,
  TraceView,
  ViewQuery,
} from "@/lib/server/entities/traces";
import { projectPath } from "@/lib/slug";
import {
  SortDirection,
  SpanKind,
  SpanStatus,
  TraceColumn,
  TraceSort,
} from "@/lib/types/enums";

import {
  MAX_DAYS,
  compact,
  listHref,
  many,
  one,
  oneOf,
  sortAfterClick,
  sortInForce,
  whole,
  type Column,
  type SearchParams,
} from "./list-query";

// A list's URL is its state — the trace list's, and the sessions' and the
// users', which take its filters and pages: every filter, the sort, the
// window and the page are query parameters, so a link holds them
// (`list-query.ts`). Two more are the trace list's own and never reach the
// server: the table or the charts, and the saved view it was opened from.
// What this module does is read that URL, write it with one thing changed,
// and put the server's numbers into words.

/** The lists a project's spans are read as, each a page of the console. */
export type ListName = "traces" | "sessions" | "users";

/** The rows a page offers to hold. */
export const ROWS_PER_PAGE = [25, 50, 100] as const;
/** The rows a page holds when the URL names none. */
export const DEFAULT_ROWS = 50;
/** The furthest page the server turns to. */
export const MAX_PAGE = 1_000;

/** The filters every list takes, each a list of values any of which admits a
 *  run, in the order the sidebar shows them. */
export const FILTERS = [
  "agent",
  "status",
  "model",
  "tool",
  "user",
  "session",
  "environment",
  "release",
  "tag",
  "prompt",
] as const;
export type FilterKey = (typeof FILTERS)[number];

/** The word the page gives each filter. */
export const FILTER_LABEL: Record<FilterKey, string> = {
  agent: "Agent",
  status: "Status",
  model: "Model",
  tool: "Tool",
  user: "User",
  session: "Session",
  environment: "Environment",
  release: "Release",
  tag: "Tag",
  prompt: "Prompt",
};

/** The filters whose values are ids, or pairs, too many to tick from a list
 *  alone: the sidebar offers them typed in as well, a tag as `key:value`. */
export const TYPED_FILTERS: readonly FilterKey[] = ["user", "session", "tag"];

/** The longest value each filter takes, in characters, as the server caps it
 *  (`crates/telemetry/src/contract.rs`); a tag's is its key, colon and value. */
const VALUE_MAX: Record<Exclude<FilterKey, "status">, number> = {
  agent: 128,
  model: 128,
  tool: 145,
  user: 128,
  session: 200,
  environment: 128,
  release: 128,
  tag: 64 + 1 + 256,
  prompt: 1_024,
};

/** The page's own state beside the list's query: never sent to the server. */
export type TraceUi = {
  /** The charts in place of the table; the table when left out. */
  display?: "chart";
  /** The saved view the page was opened from. */
  view?: string;
};

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** A list left empty, as the URL leaves a filter out. */
function some<T>(values: T[]): T[] | undefined {
  return values.length > 0 ? values : undefined;
}

/** Whether `value` is within `filter`'s length, counted in characters as the
 *  server counts them, not in UTF-16's units. */
export function fits(filter: Exclude<FilterKey, "status">, value: string): boolean {
  return [...value].length <= VALUE_MAX[filter];
}

/** A filter's values from the URL, but those past its length. */
function capped(filter: Exclude<FilterKey, "status">, searchParams: SearchParams): string[] {
  return many(searchParams[filter]).filter((value) => fits(filter, value));
}

/** The filters the URL carries, every value the server would refuse dropped
 *  — a status that is no status, a value past its length — so a stale or
 *  hand-edited link opens the list, not a 400. */
export function parseFilters(searchParams: SearchParams): ListFilters {
  const statuses = Object.values(SpanStatus) as string[];
  return compact({
    agent: some(capped("agent", searchParams)),
    user: some(capped("user", searchParams)),
    session: some(capped("session", searchParams)),
    status: some(
      many(searchParams.status).filter((s): s is SpanStatus => statuses.includes(s)),
    ),
    model: some(capped("model", searchParams)),
    tool: some(capped("tool", searchParams)),
    environment: some(capped("environment", searchParams)),
    release: some(capped("release", searchParams)),
    tag: some(capped("tag", searchParams).filter(isTagPair)),
    prompt: some(capped("prompt", searchParams)),
  });
}

/** A list's query as the URL carries it, its sort among `sorts`: the
 *  filters, the window, and rows a page offers; the first page is the URL's
 *  when it names none. */
export function parseListQuery<S extends string>(
  searchParams: SearchParams,
  sorts: Record<string, S>,
): ListQuery<S> {
  const rows = whole(searchParams.limit, 100);
  const page = whole(searchParams.page, MAX_PAGE);
  return compact({
    ...parseFilters(searchParams),
    sort: oneOf(searchParams.sort, sorts),
    dir: oneOf(searchParams.dir, SortDirection),
    days: whole(searchParams.days, MAX_DAYS),
    limit: rows !== undefined && (ROWS_PER_PAGE as readonly number[]).includes(rows) ? rows : undefined,
    page: page !== undefined && page > 1 ? page : undefined,
  });
}

/** The trace list's query the URL carries. */
export function parseTraceQuery(searchParams: SearchParams): TraceQuery {
  return parseListQuery(searchParams, TraceSort);
}

/** Whether `value` is a tag as the filter takes one, `key:value` with a key
 *  of 1 to 64 characters and a value up to 256, as ingest keeps a tag. */
export function isTagPair(value: string): boolean {
  const colon = value.indexOf(":");
  if (colon < 1) return false;
  return [...value.slice(0, colon)].length <= 64 && [...value.slice(colon + 1)].length <= 256;
}

/** The page's own state from its URL. */
export function parseTraceUi(searchParams: SearchParams): TraceUi {
  const view = one(searchParams.view);
  return compact({
    display: one(searchParams.display) === "chart" ? ("chart" as const) : undefined,
    view: view !== undefined && UUID.test(view) ? view : undefined,
  });
}

/** The list's path with `query` in its URL: `patch` changes some keys, and a
 *  key patched to `undefined` is dropped; `ui`, the page's own, rides along. */
export function tracesHref(
  organization: string,
  project: string,
  query: TraceQuery,
  patch: Partial<TraceQuery> = {},
  ui: TraceUi = {},
): string {
  return listHref(organization, project, "/traces", { ...query, ...patch, ...ui });
}

/** `list`'s path with `query` in its URL, as the parts every list shares —
 *  its filters, its pages — write it; `ui` is the trace list's own. */
export function listPageHref(
  list: ListName,
  organization: string,
  project: string,
  query: ListQuery<string>,
  ui: TraceUi = {},
): string {
  return listHref(organization, project, `/${list}`, { ...query, ...ui });
}

/** A trace's own page. */
export function traceHref(organization: string, project: string, runId: string): string {
  return projectPath(organization, project, `/traces/${encodeURIComponent(runId)}`);
}

/** The list's key for the run its peek holds open (`components/traces/peek.tsx`):
 *  the browser's, never the server's, so the list is not read again for it. */
export const PEEK_PARAM = "peek";

/** Sent on the window when what a peek holds open may have new spans, held as
 *  a page's nudges are (`components/realtime-listener.tsx`). */
export const PEEK_NUDGED = "telmoni:peek-nudged";

/** The query with `value` ticked under `filter`, or unticked where it was,
 *  from the first page again: what clicking a value does. */
export function toggled<Q extends ListQuery<string>>(query: Q, filter: FilterKey, value: string): Q {
  const ticked: string[] = query[filter] ?? [];
  const values = ticked.includes(value)
    ? ticked.filter((v) => v !== value)
    : [...ticked, value];
  return compact({ ...query, [filter]: some(values), page: undefined } as Q);
}

/** The query without `value` under `filter`, or without the filter where no
 *  value is named, from the first page again. */
export function without<Q extends ListQuery<string>>(query: Q, filter: FilterKey, value?: string): Q {
  const kept = value === undefined ? [] : (query[filter] ?? []).filter((v) => v !== value);
  return compact({ ...query, [filter]: some(kept as string[]), page: undefined } as Q);
}

/** The query with no filter: its sort, window and rows kept. */
export function unfiltered<Q extends ListQuery<string>>(query: Q): Q {
  return compact({ sort: query.sort, dir: query.dir, days: query.days, limit: query.limit } as Q);
}

/** One value of one filter in force, as the page shows it. */
export type ActiveFilter = { filter: FilterKey; label: string; value: string; text: string };

/** The filters in force, a value at a time, in the sidebar's order. */
export function activeFilters(query: ListQuery<string>): ActiveFilter[] {
  return FILTERS.flatMap((filter) =>
    (query[filter] ?? []).map((value) => ({
      filter,
      label: FILTER_LABEL[filter],
      value,
      text: filter === "status" ? STATUS_LABEL[value as SpanStatus] : value,
    })),
  );
}

/** Where the columns menu files a column: the counts of a run's parts, and its tokens. */
export type ColumnGroup = "counts" | "tokens";

/** Each group's heading in the columns menu. */
export const COLUMN_GROUP_LABEL: Record<ColumnGroup, string> = {
  counts: "Counts",
  tokens: "Tokens",
};

/** A column of a list a person may show or hide: its heading, its sort
 *  when it has one and the way it sorts when first clicked, whether it holds
 *  a number, which stands right, the group the columns menu files it under,
 *  and what it means, where its heading does not say, behind an ⓘ there. */
export type ColumnSpec<K extends string = TraceColumn, S extends string = TraceSort> = {
  key: K;
  label: string;
  sort?: S;
  first?: SortDirection;
  numeric?: boolean;
  group?: ColumnGroup;
  hint?: string;
};

/** What a list's columns menu offers: the id every row shows, ticked and not
 *  to be unticked, the columns a person shows or hides, and those it shows
 *  until they pick others. */
export type ListTable<K extends string = string, S extends string = string> = {
  id: { label: string; hint: string };
  columns: readonly ColumnSpec<K, S>[];
  defaults: readonly K[];
};

/** The trace's id, which every row shows, since it opens the trace: in the
 *  columns menu, ticked and not to be unticked. */
export const TRACE_ID_COLUMN = {
  label: "Trace ID",
  hint: "The run's id, which opens its trace. Always shown.",
} as const;

/** Every column but the trace's id, which always shows, in the table's
 *  order: the newest, the longest and the most first; names in the
 *  alphabet's order. */
export const TRACE_COLUMNS: readonly ColumnSpec[] = [
  { key: TraceColumn.Started, label: "Started", sort: TraceSort.Started, first: SortDirection.Desc },
  {
    key: TraceColumn.Ended,
    label: "Ended",
    hint: "When the run's last span ended; Running until the run's own span arrives.",
  },
  { key: TraceColumn.Agent, label: "Agent", sort: TraceSort.Agent, first: SortDirection.Asc },
  {
    key: TraceColumn.User,
    label: "User",
    sort: TraceSort.User,
    first: SortDirection.Asc,
    hint: "The end user the run was for, as stored: hashed under the project's own key.",
  },
  {
    key: TraceColumn.Session,
    label: "Session",
    sort: TraceSort.Session,
    first: SortDirection.Asc,
    hint: "The session the run belongs to, gen_ai.conversation.id.",
  },
  {
    key: TraceColumn.Environment,
    label: "Environment",
    hint: "The deployment the run's spans were sent from, deployment.environment.name on their resource; each once where the run crossed services deployed apart.",
  },
  {
    key: TraceColumn.Release,
    label: "Release",
    hint: "The version or commit the run's spans were sent from, service.version on their resource; each once where the run crossed services.",
  },
  {
    key: TraceColumn.Status,
    label: "Status",
    sort: TraceSort.Status,
    first: SortDirection.Asc,
    hint: "OK, Error or Cancelled once the run's own span arrives; Running until it does. Beside any but Error, how many of the run's spans failed.",
  },
  {
    key: TraceColumn.ErrorType,
    label: "Error",
    hint: "The class of the error the run ended on, never its message: TimeoutError, RateLimitError.",
  },
  {
    key: TraceColumn.Steps,
    label: "Steps",
    sort: TraceSort.Steps,
    first: SortDirection.Desc,
    numeric: true,
    group: "counts",
  },
  {
    key: TraceColumn.ModelCalls,
    label: "Model calls",
    sort: TraceSort.ModelCalls,
    first: SortDirection.Desc,
    numeric: true,
    group: "counts",
  },
  {
    key: TraceColumn.ToolCalls,
    label: "Tool calls",
    sort: TraceSort.ToolCalls,
    first: SortDirection.Desc,
    numeric: true,
    group: "counts",
  },
  {
    key: TraceColumn.Errors,
    label: "Errors",
    sort: TraceSort.Errors,
    first: SortDirection.Desc,
    numeric: true,
    group: "counts",
    hint: "How many of the run's spans failed.",
  },
  {
    key: TraceColumn.Spans,
    label: "Spans",
    numeric: true,
    group: "counts",
    hint: "Every span the run sent: its own, its steps, its calls and any other.",
  },
  {
    key: TraceColumn.InputTokens,
    label: "Input tokens",
    sort: TraceSort.InputTokens,
    first: SortDirection.Desc,
    numeric: true,
    group: "tokens",
  },
  {
    key: TraceColumn.OutputTokens,
    label: "Output tokens",
    sort: TraceSort.OutputTokens,
    first: SortDirection.Desc,
    numeric: true,
    group: "tokens",
  },
  {
    key: TraceColumn.CacheReadTokens,
    label: "Cache read",
    numeric: true,
    group: "tokens",
    hint: "Input tokens the run's model calls read from a provider's prompt cache.",
  },
  {
    key: TraceColumn.CacheWriteTokens,
    label: "Cache write",
    numeric: true,
    group: "tokens",
    hint: "Input tokens the run's model calls wrote to a provider's prompt cache.",
  },
  {
    key: TraceColumn.Tokens,
    label: "Tokens",
    sort: TraceSort.Tokens,
    first: SortDirection.Desc,
    numeric: true,
    group: "tokens",
    hint: "Input and output tokens, summed.",
  },
  {
    key: TraceColumn.Duration,
    label: "Duration",
    sort: TraceSort.Duration,
    first: SortDirection.Desc,
    numeric: true,
    hint: "From the run's first span's start to its last span's end.",
  },
  { key: TraceColumn.Models, label: "Models" },
  { key: TraceColumn.Tools, label: "Tools" },
];

/** A row of the columns menu: a column, or a group of them under a heading
 *  that opens and closes, where the group's columns stand together. */
export type ColumnBlock<K extends string = string, S extends string = string> =
  | { kind: "column"; column: ColumnSpec<K, S> }
  | { kind: "group"; group: ColumnGroup; label: string; columns: ColumnSpec<K, S>[] };

/** A list's columns menu, in the table's order, each group where its first
 *  column falls. */
export function columnMenu<K extends string, S extends string>(
  columns: readonly ColumnSpec<K, S>[],
): ColumnBlock<K, S>[] {
  return columns.reduce<ColumnBlock<K, S>[]>((blocks, column) => {
    const last = blocks.at(-1);
    if (!column.group) {
      blocks.push({ kind: "column", column });
    } else if (last?.kind === "group" && last.group === column.group) {
      last.columns.push(column);
    } else {
      blocks.push({
        kind: "group",
        group: column.group,
        label: COLUMN_GROUP_LABEL[column.group],
        columns: [column],
      });
    }
    return blocks;
  }, []);
}

/** `keys` as a list's table shows them: each one of `columns`, once, in the
 *  list's order, whatever order they were picked in. */
export function ordered<K extends string>(
  columns: readonly ColumnSpec<K, string>[],
  keys: readonly string[],
): K[] {
  return columns.filter((c) => keys.includes(c.key)).map((c) => c.key);
}

/** The sortable columns of a list, as the shared sort reads them. */
export function sortsOf<S extends string>(columns: readonly ColumnSpec<string, S>[]): Column<S>[] {
  return columns.flatMap((c) =>
    c.sort && c.first ? [{ sort: c.sort, label: c.label, first: c.first }] : [],
  );
}

/** `columns` as the trace table shows them. */
export function orderedColumns(columns: readonly string[]): TraceColumn[] {
  return ordered(TRACE_COLUMNS, columns);
}

/** The columns a list shows until a person picks others: what it showed
 *  before it offered the rest. */
export const DEFAULT_COLUMNS: readonly TraceColumn[] = [
  TraceColumn.Started,
  TraceColumn.Agent,
  TraceColumn.User,
  TraceColumn.Session,
  TraceColumn.Status,
  TraceColumn.Steps,
  TraceColumn.Tokens,
  TraceColumn.Duration,
];

/** The trace list's columns menu. */
export const TRACE_TABLE: ListTable<TraceColumn, TraceSort> = {
  id: TRACE_ID_COLUMN,
  columns: TRACE_COLUMNS,
  defaults: DEFAULT_COLUMNS,
};

/** The trace list's columns menu, in its rows. */
export const COLUMN_MENU: readonly ColumnBlock<TraceColumn, TraceSort>[] = columnMenu(TRACE_COLUMNS);

/** The sortable columns, as the shared sort reads them. */
const SORTS = sortsOf(TRACE_COLUMNS);

/** The sort in force: the URL's, else the server's default, newest first. */
export function currentSort(query: TraceQuery): { sort: TraceSort; dir: SortDirection } {
  return sortInForce(SORTS, TraceSort.Started, query);
}

/** What clicking a column heading asks for: the other way when the list is
 *  already sorted by it, its own first way otherwise, from the first page. */
export function nextSort(
  query: TraceQuery,
  column: TraceSort,
): { sort: TraceSort; dir: SortDirection; page: undefined } {
  return { ...sortAfterClick(SORTS, TraceSort.Started, query, column), page: undefined };
}

/** A ready-made query a toolbar button applies; a cost preset waits for the list to read spend. */
export type Preset = {
  id: "errors" | "slow" | "tokens";
  label: string;
  hint: string;
};

export const TRACE_PRESETS: readonly Preset[] = [
  { id: "errors", label: "Errors", hint: "Only the runs that ended in error" },
  { id: "slow", label: "Slow", hint: "The longest runs first" },
  { id: "tokens", label: "Tokens", hint: "The runs that used the most tokens first" },
];

/** Whether the list shows what `preset` asks for. */
export function presetActive(query: TraceQuery, preset: Preset["id"]): boolean {
  const { sort, dir } = currentSort(query);
  switch (preset) {
    case "errors":
      return query.status?.length === 1 && query.status[0] === SpanStatus.Error;
    case "slow":
      return sort === TraceSort.Duration && dir === SortDirection.Desc;
    case "tokens":
      return sort === TraceSort.Tokens && dir === SortDirection.Desc;
  }
}

/** The query with `preset` applied, or taken off where it was: the errors
 *  filter cleared, or the sort set back to the newest first. */
export function presetToggled(query: TraceQuery, preset: Preset["id"]): TraceQuery {
  const on = presetActive(query, preset);
  switch (preset) {
    case "errors":
      return compact({ ...query, status: on ? undefined : [SpanStatus.Error], page: undefined });
    case "slow":
    case "tokens": {
      const sort = preset === "slow" ? TraceSort.Duration : TraceSort.Tokens;
      return compact({
        ...query,
        sort: on ? undefined : sort,
        dir: on ? undefined : SortDirection.Desc,
        page: undefined,
      });
    }
  }
}

/** How many pages the list's total fills at `limit` rows a page: one at
 *  least, so an empty list is on its only page. */
export function pageCount(total: number, limit: number): number {
  return Math.max(1, Math.min(MAX_PAGE, Math.ceil(total / Math.max(1, limit))));
}

/** Where each list's download is served: the sessions' beside their page
 *  rather than under it, where a session's id would be read as one. */
const DOWNLOAD_PATH: Record<ListName, string> = {
  traces: "/traces/export",
  sessions: "/session-export",
  users: "/users/export",
};

/** The trace list as a file, through the console's own route: its filters
 *  and sort, or the runs `runs` names alone. */
export function downloadHref(
  organization: string,
  project: string,
  query: TraceQuery,
  format: DownloadFormat,
  runs: readonly string[] = [],
): string {
  return listDownloadHref("traces", organization, project, query, format, runs);
}

/** `list` as a file, through the console's own route: its filters and sort,
 *  or — the trace list's alone — the runs `runs` names. */
export function listDownloadHref(
  list: ListName,
  organization: string,
  project: string,
  query: ListQuery<string>,
  format: DownloadFormat,
  runs: readonly string[] = [],
): string {
  return listHref(organization, project, DOWNLOAD_PATH[list], {
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
    sort: query.sort,
    dir: query.dir,
    days: query.days,
    format,
    run: runs,
  });
}

/** What a saved view keeps of the list's query: all of it but the page. */
export function viewQueryOf(query: TraceQuery): ViewQuery {
  return compact({
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
    sort: query.sort,
    dir: query.dir,
    days: query.days,
    limit: query.limit,
  });
}

/** The list's query a saved view opens on, every value the page would drop
 *  dropped, as it drops a stale URL's. */
export function queryOfView(view: ViewQuery): TraceQuery {
  return parseTraceQuery({
    agent: view.agent,
    user: view.user,
    session: view.session,
    status: view.status,
    model: view.model,
    tool: view.tool,
    environment: view.environment,
    release: view.release,
    tag: view.tag,
    prompt: view.prompt,
    sort: view.sort,
    dir: view.dir,
    days: view.days === undefined ? undefined : String(view.days),
    limit: view.limit === undefined ? undefined : String(view.limit),
  });
}

/** The list as a saved view opens it, the view named in the URL. */
export function viewHref(organization: string, project: string, view: TraceView, ui: TraceUi = {}): string {
  return tracesHref(organization, project, queryOfView(view.query), {}, { ...ui, view: view.id });
}

/** Whether the list shows what `view` saved: its query, the page aside, a
 *  filter's values in any order. */
export function showsView(query: TraceQuery, view: TraceView): boolean {
  const saved = queryOfView(view.query);
  const sameValues = (a?: readonly string[], b?: readonly string[]) =>
    [...(a ?? [])].sort().join("\u0000") === [...(b ?? [])].sort().join("\u0000");
  const a = currentSort(query);
  const b = currentSort(saved);
  return (
    FILTERS.every((filter) => sameValues(query[filter], saved[filter])) &&
    a.sort === b.sort &&
    a.dir === b.dir &&
    query.days === saved.days &&
    query.limit === saved.limit
  );
}

// The console's own language, not the browser's: a count drawn on the server
// is drawn again in the browser as the page goes live, and grouped there the
// reader's way it would read otherwise ("12.345"), which React refuses as a
// mismatch and draws the page again for.
const COUNT = new Intl.NumberFormat("en-US");

/** A count, grouped as the console's English groups it: `12,345`. */
export function formatCount(n: number): string {
  return COUNT.format(n);
}

const DOLLARS = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const UNDER_A_DOLLAR = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  minimumSignificantDigits: 2,
  maximumSignificantDigits: 3,
});

/** A cost in US dollars: to the cent from a dollar, else to three figures, since
 *  a call's is often a fraction of a cent (`$12.34`, `$0.0123`, `$0.000045`). */
export function formatCost(dollars: number): string {
  // Rounded first, so $0.9999 reads as $1.00 rather than $1.0.
  const shown = Number(dollars.toPrecision(3));
  return shown >= 1 || shown === 0 ? DOLLARS.format(dollars) : UNDER_A_DOLLAR.format(dollars);
}

/** A duration in milliseconds, in the unit a person reads it in (`340 ms`, `1.2 s`,
 *  `2 min 3 s`, `1 h 4 min`), rounded before the unit is chosen: never `60 s`. */
export function formatDuration(ms: number): string {
  if (ms < 0) ms = 0;
  if (Math.round(ms) < 1_000) return `${Math.round(ms)} ms`;
  const tenths = Math.round(ms / 100);
  if (tenths < 600) return `${(tenths / 10).toString()} s`;
  const seconds = Math.round(ms / 1_000);
  if (seconds < 3_600) {
    const minutes = Math.floor(seconds / 60);
    const rest = seconds % 60;
    return rest ? `${minutes} min ${rest} s` : `${minutes} min`;
  }
  const minutes = Math.round(ms / 60_000);
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest ? `${hours} h ${rest} min` : `${hours} h`;
}

/** The most rows one download holds, as the server writes them. */
export const DOWNLOAD_MAX = 10_000;

/** What each list's rows are, one and many, as the page counts them. */
export const ROW_NOUN: Record<ListName, readonly [string, string]> = {
  traces: ["trace", "traces"],
  sessions: ["session", "sessions"],
  users: ["user", "users"],
};

/** The word the console gives each status. */
export const STATUS_LABEL: Record<SpanStatus, string> = {
  [SpanStatus.Ok]: "OK",
  [SpanStatus.Error]: "Error",
  [SpanStatus.Cancelled]: "Cancelled",
};

/** The word the console gives each kind of span. */
export const KIND_LABEL: Record<SpanKind, string> = {
  [SpanKind.Run]: "Run",
  [SpanKind.Step]: "Step",
  [SpanKind.ModelCall]: "Model call",
  [SpanKind.ToolCall]: "Tool call",
  [SpanKind.Span]: "Span",
};

/** A run's id as a row shows it: its first eight characters, enough to tell
 *  two apart, the whole on the trace's own page. */
export function shortId(id: string): string {
  return id.slice(0, 8);
}

/** The milliseconds between two of the server's times. */
export function spanMillis(span: Pick<TraceSpan, "started_at" | "ended_at">): number {
  return Math.max(0, new Date(span.ended_at).getTime() - new Date(span.started_at).getTime());
}

/** Where a span sits on the trace's timeline, as shares of its whole: what
 *  the timeline view draws, every pixel from the spans' own times. */
export function timelineBars(
  spans: TraceSpan[],
): { span: TraceSpan; offset: number; width: number; depth: number }[] {
  if (spans.length === 0) return [];
  const starts = spans.map((s) => new Date(s.started_at).getTime());
  const ends = spans.map((s) => new Date(s.ended_at).getTime());
  const first = Math.min(...starts);
  const last = Math.max(...ends);
  const whole = Math.max(1, last - first);
  const byId = new Map(spans.map((s) => [s.span_id, s]));
  const depthOf = (span: TraceSpan): number => {
    let depth = 0;
    let parent = byId.get(span.parent_span_id);
    const seen = new Set<string>();
    while (parent && !seen.has(parent.span_id) && depth < 32) {
      depth += 1;
      seen.add(parent.span_id);
      parent = byId.get(parent.parent_span_id);
    }
    return depth;
  };
  return spans.map((span, i) => ({
    span,
    offset: ((starts[i]! - first) / whole) * 100,
    // At least a sliver, so a span of no duration is still seen.
    width: Math.max(0.5, ((ends[i]! - starts[i]!) / whole) * 100),
    depth: depthOf(span),
  }));
}
