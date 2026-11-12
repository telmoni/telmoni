import type { Change } from "@/lib/server/entities/changes";
import type {
  DownloadFormat,
  ListFilters,
  ListQuery,
  TraceQuery,
  TraceSpan,
} from "@/lib/server/entities/traces";
import { isOneOf } from "@/lib/one-of";
import { projectPath } from "@/lib/slug";
import { ChangeKind, SortDirection, SpanFault, SpanKind, SpanStatus, TraceSort } from "@/lib/types/enums";
import {
  FILTER_KEYS,
  TAG_KEY_MAX,
  TAG_VALUE_MAX,
  TraceColumn,
  filtersIn,
  readTagSort,
  type TagSort,
} from "@/lib/types/traces";

import {
  MAX_DAYS,
  compact,
  listHref,
  many,
  one,
  oneOf,
  sortAfterClick,
  sortInForce,
  type Column,
  type SearchParams,
  whole,
} from "./list-query";

// The trace list's URL is its state: every filter, the sort, the window and
// the page are query parameters, so a link holds them (`list-query.ts`).
// What this module does is read that URL, write it with one thing changed,
// and put the server's numbers into words. The sessions list and the users
// list are built from the same parts (`sessions.ts`, `users.ts`).

/** A list a project's spans are read as, each a page of the console. */
export type ListName = "traces" | "sessions" | "users";

/** A call's tokens as they are kept: input with the cache's reads and
 *  writes, output with reasoning. */
export type TokenCounts = {
  input_tokens: number;
  output_tokens: number;
  cache_read_input_tokens: number;
  cache_write_input_tokens: number;
  reasoning_output_tokens: number;
};

/** One bar's tokens, by the model that spent them. */
export type TokenBar = { at: string; tokens_by_model: Record<string, TokenCounts> };
export type OutputSpeedBar = {
  at: string;
  output_speed_by_model: Record<string, { p50: number | null; p05: number | null }>;
};

/** The counts summed and told apart as five that add up to input and output,
 *  and the share of input the cache answered, none where there was no input.
 *  A part past its whole leaves none of the rest, as the price reads it. */
export function tokenBreakdown(counts: readonly TokenCounts[]) {
  const sum = (key: keyof TokenCounts) => counts.reduce((total, count) => total + count[key], 0);
  const inputTotal = sum("input_tokens");
  const outputTotal = sum("output_tokens");
  const cacheRead = sum("cache_read_input_tokens");
  const cacheWrite = sum("cache_write_input_tokens");
  const reasoning = sum("reasoning_output_tokens");
  return {
    input: Math.max(0, inputTotal - cacheRead - cacheWrite),
    cacheRead,
    cacheWrite,
    output: Math.max(0, outputTotal - reasoning),
    reasoning,
    inputTotal,
    outputTotal,
    cacheShare: inputTotal === 0 ? null : cacheRead / inputTotal,
  };
}

// The console's own language, as `COUNT` is: the share is drawn on the server too.
const SHARE = new Intl.NumberFormat("en-US", { style: "percent", maximumFractionDigits: 1 });

/** The share of input the cache answered, a dash where there was no input. */
export function formatCacheShare(share: number | null): string {
  return share === null ? "—" : SHARE.format(share);
}

/** The rows a page offers to hold. */
export const ROWS_PER_PAGE = [25, 50, 100] as const;
/** The rows a page holds when the URL names none. */
export const DEFAULT_ROWS = 50;
/** The furthest page the server turns to. */
export const MAX_PAGE = 1_000;

/** The filters every list takes, each a list of values any of which admits a
 *  trace, in the order the sidebar shows them: the keys of the one schema. */
export const FILTERS = FILTER_KEYS;
export type FilterKey = (typeof FILTERS)[number];

export const FAULT_LABEL: Record<SpanFault, string> = {
  rate_limited: "Rate limited",
  overloaded: "Overloaded",
  timeout: "Timeout",
  context_length: "Context length",
  unauthorized: "Unauthorized",
  invalid_output: "Invalid output",
  refused: "Refused",
  truncated: "Truncated",
  other: "Other",
};

/** The word the page gives each filter. */
export const FILTER_LABEL: Record<FilterKey, string> = {
  agent: "Agent",
  status: "Status",
  fault: "Fault",
  model: "Model",
  tool: "Tool",
  user: "User",
  session: "Session",
  environment: "Environment",
  release: "Release",
  tag: "Tag",
  prompt: "Prompt",
  prompt_version: "Prompt version",
};

/** The filters whose values are stored hashed: one is typed as the
 *  application sent it, and the server answers what it is stored as. */
export const STORED_ID_FILTERS = ["user", "session"] as const satisfies readonly FilterKey[];
export type StoredIdFilter = (typeof STORED_ID_FILTERS)[number];

/** The filters whose values are ids, or pairs, too many to tick from a list
 *  alone: the sidebar offers them typed in as well, an id as the application
 *  sent it and a tag as `key:value`. */
export const TYPED_FILTERS: readonly FilterKey[] = [...STORED_ID_FILTERS, "tag"];

/** The filters a list's sidebar shows before its More is opened: what most
 *  narrowing starts from. The rest show once opened, or while one is ticked. */
export const PRIMARY_FILTERS: readonly FilterKey[] = ["agent", "status", "fault", "model", "environment"];

/** The longest value each filter takes, in characters, as the server caps it
 *  (`crates/telemetry/src/contract.rs`); a tag's is its key, colon and value,
 *  a prompt version's its name, colon and version. */
export const FILTER_VALUE_MAX: Record<Exclude<FilterKey, "status" | "fault">, number> = {
  agent: 128,
  model: 128,
  tool: 145,
  user: 128,
  session: 200,
  environment: 128,
  release: 128,
  tag: TAG_KEY_MAX + 1 + TAG_VALUE_MAX,
  prompt: 1_024,
  prompt_version: 1_024 + 1 + 1_024,
};

/** A list left empty, as the URL leaves a filter out. */
function some<T>(values: T[]): T[] | undefined {
  return values.length > 0 ? values : undefined;
}

/** Whether `value` is within `filter`'s length, counted in characters as the
 *  server counts them, not in UTF-16's units. */
export function fits(filter: Exclude<FilterKey, "status">, value: string): boolean {
  if (filter === "fault") return isOneOf(Object.values(SpanFault), value);
  return [...value].length <= FILTER_VALUE_MAX[filter];
}

/** A filter's values from the URL, but those past its length. */
function capped(filter: Exclude<FilterKey, "status">, searchParams: SearchParams): string[] {
  return many(searchParams[filter]).filter((value) => fits(filter, value));
}

/** The filters the URL carries, every value the server would refuse dropped
 *  — a status that is no status, a value past its length — so a stale or
 *  hand-edited link opens the list, not a 400. */
export function parseFilters(searchParams: SearchParams): ListFilters {
  const statuses = Object.values(SpanStatus);
  const values = (filter: FilterKey): string[] => {
    if (filter === "status") return many(searchParams.status).filter((s) => isOneOf(statuses, s));
    if (filter === "fault") return many(searchParams.fault).filter((s) => isOneOf(Object.values(SpanFault), s));
    const kept = capped(filter, searchParams);
    if (filter === "tag") return kept.filter(isTagPair);
    return filter === "prompt_version" ? kept.filter(isPromptVersion) : kept;
  };
  return compact(
    Object.fromEntries(FILTERS.map((filter) => [filter, some(values(filter))])) as ListFilters,
  );
}

/** A list's query as the URL carries it, its sort among `sorts` or one
 *  `readSort` reads: the filters, the window, and rows a page offers; the
 *  first page is the URL's when it names none. */
export function parseListQuery<S extends string>(
  searchParams: SearchParams,
  sorts: Record<string, S>,
  readSort?: (text: string) => S | undefined,
): ListQuery<S> {
  const rows = whole(searchParams.limit, 100);
  const page = whole(searchParams.page, MAX_PAGE);
  const sort = one(searchParams.sort);
  const score = one(searchParams.score)?.trim();
  const named = score && [...score].length <= SCORE_NAME_MAX ? score : undefined;
  const bound = (given: string | string[] | undefined) => {
    const n = Number(one(given));
    return named && one(given) !== undefined && Number.isFinite(n) ? n : undefined;
  };
  return compact({
    ...parseFilters(searchParams),
    score: named,
    score_min: bound(searchParams.score_min),
    score_max: bound(searchParams.score_max),
    sort: oneOf(searchParams.sort, sorts) ?? (sort === undefined ? undefined : readSort?.(sort)),
    dir: oneOf(searchParams.dir, SortDirection),
    days: whole(searchParams.days, MAX_DAYS),
    limit: isOneOf(ROWS_PER_PAGE, rows) ? rows : undefined,
    page: page !== undefined && page > 1 ? page : undefined,
  });
}

/** The trace list's query the URL carries. */
export function parseTraceQuery(searchParams: SearchParams): TraceQuery {
  return TRACES.parse(searchParams);
}

/** Whether `value` is a tag as the filter takes one, `key:value` with a key
 *  of 1 to 64 characters and a value up to 256, as ingest keeps a tag. */
export function isTagPair(value: string): boolean {
  const colon = value.indexOf(":");
  if (colon < 1) return false;
  return (
    [...value.slice(0, colon)].length <= TAG_KEY_MAX &&
    [...value.slice(colon + 1)].length <= TAG_VALUE_MAX
  );
}

/** Whether `value` is a prompt's version as the filter takes one,
 *  `name:version`, split at its first colon and neither part empty. */
function isPromptVersion(value: string): boolean {
  const colon = value.indexOf(":");
  return colon > 0 && colon < value.length - 1;
}

/** The keys among `pairs`, each `key:value` as a tag's facet names it, split
 *  at its first colon as the filter splits one: each once, in order. */
export function tagKeys(pairs: readonly string[]): string[] {
  return [...new Set(pairs.filter(isTagPair).map((pair) => pair.slice(0, pair.indexOf(":"))))].sort();
}

/** The values `key` takes among `pairs`, each once, in order. */
export function tagValues(pairs: readonly string[], key: string): string[] {
  const values = pairs
    .filter((pair) => isTagPair(pair) && pair.slice(0, pair.indexOf(":")) === key)
    .map((pair) => pair.slice(pair.indexOf(":") + 1));
  return [...new Set(values)].sort();
}

/** The list's path with `query` in its URL: `patch` changes some keys, and a
 *  key patched to `undefined` is dropped. */
export function tracesHref(
  organization: string,
  project: string,
  query: TraceQuery,
  patch: Partial<TraceQuery> = {},
): string {
  return listHref(organization, project, "/traces", query, patch);
}

/** `list`'s path with `query` in its URL, as the parts every list shares —
 *  its filters, its pages — write it. */
export function listPageHref(
  list: ListName,
  organization: string,
  project: string,
  query: ListQuery<string>,
): string {
  return listHref(organization, project, `/${list}`, query);
}

/** A trace's own page. */
export function traceHref(organization: string, project: string, traceId: string): string {
  return projectPath(organization, project, `/traces/${encodeURIComponent(traceId)}`);
}

/** A trace opened in the peek over the Traces list of `listHref`, so the
 *  list stands around it and its neighbours are a step away. */
export function peekHref(listHref: string, traceId: string): string {
  return `${listHref}${listHref.includes("?") ? "&" : "?"}${PEEK_PARAM}=${encodeURIComponent(traceId)}`;
}

/** A trace's spans, through the console's own route. */
export function traceReadHref(traceId: string, projectId: string): string {
  return `/api/v1/traces/${encodeURIComponent(traceId)}?project=${encodeURIComponent(projectId)}`;
}

/** The list's key for the trace its peek holds open (`components/traces/peek.tsx`):
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
export function without<Q extends ListQuery<string>>(
  query: Q,
  filter: FilterKey | "score",
  value?: string,
): Q {
  if (filter === "score") {
    return compact({ ...query, score: undefined, score_min: undefined, score_max: undefined, page: undefined });
  }
  const kept = value === undefined ? [] : (query[filter] ?? []).filter((v) => v !== value);
  return compact({ ...query, [filter]: some(kept as string[]), page: undefined } as Q);
}

/** The query with no filter: its sort, window and rows kept. */
export function unfiltered<Q extends ListQuery<string>>(query: Q): Q {
  return compact({ sort: query.sort, dir: query.dir, days: query.days, limit: query.limit } as Q);
}

/** One value of one filter in force, as the page shows it: a facet's, or
 *  the score's. */
export type ActiveFilter = { filter: FilterKey | "score"; label: string; value: string; text: string };

/** The longest score name the server keeps, in characters. */
export const SCORE_NAME_MAX = 64;

/** The longest name a saved dashboard goes by. */
export const VIEW_NAME_MAX = 60;

/** The longest span name the server keeps: an operation, a space and a model. */
export const SPAN_NAME_MAX = 145;

/** The filters in force, a value at a time, in the sidebar's order, the
 *  score first. */
export function activeFilters(query: ListQuery<string>): ActiveFilter[] {
  const bounds = [
    query.score_min === undefined ? null : `≥ ${query.score_min}`,
    query.score_max === undefined ? null : `≤ ${query.score_max}`,
  ].filter(Boolean);
  const scored: ActiveFilter[] =
    query.score === undefined
      ? []
      : [{ filter: "score", label: "Score", value: query.score, text: [query.score, ...bounds].join(" ") }];
  return scored.concat(
    FILTERS.flatMap((filter) =>
      (query[filter] ?? []).map((value) => ({
        filter,
        label: FILTER_LABEL[filter],
        value,
        text: filter === "status" ? STATUS_LABEL[value as SpanStatus] : value,
      })),
    ),
  );
}

/** Where the columns menu files a column: the counts of a trace's parts, and its tokens. */
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
  /** Whether its heading sorts by one tag's value, the key picked from a menu. */
  byTag?: boolean;
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
  hint: "The trace's id, which opens it. Always shown.",
} as const;

/** Every column but the trace's id, which always shows, in the table's
 *  order: the newest, the longest and the most first; names in the
 *  alphabet's order. */
export const TRACE_COLUMNS: readonly ColumnSpec[] = [
  { key: TraceColumn.Started, label: "Started", sort: TraceSort.Started, first: SortDirection.Desc },
  {
    key: TraceColumn.Ended,
    label: "Ended",
    hint: "When the trace's last span ended; Running until the agent's own span arrives.",
  },
  { key: TraceColumn.Agent, label: "Agent", sort: TraceSort.Agent, first: SortDirection.Asc },
  {
    key: TraceColumn.User,
    label: "User",
    sort: TraceSort.User,
    first: SortDirection.Asc,
    hint: "The end user the trace was for, as stored: hashed under the project's own key. It opens the user's page.",
  },
  {
    key: TraceColumn.Session,
    label: "Session",
    sort: TraceSort.Session,
    first: SortDirection.Asc,
    hint: "The session the trace belongs to, gen_ai.conversation.id, as stored: hashed under the project's own key. It opens the session's page.",
  },
  {
    key: TraceColumn.Environment,
    label: "Environment",
    hint: "The deployment the trace's spans were sent from, deployment.environment.name on their resource; each once where the trace crossed services deployed apart.",
  },
  {
    key: TraceColumn.Release,
    label: "Release",
    hint: "The version or commit the trace's spans were sent from, service.version on their resource; each once where the trace crossed services.",
  },
  {
    key: TraceColumn.Status,
    label: "Status",
    sort: TraceSort.Status,
    first: SortDirection.Asc,
    hint: "OK, Error or Cancelled once the agent's own span arrives; Running until it does. Beside any but Error, how many of the trace's spans failed.",
  },
  {
    key: TraceColumn.ErrorType,
    label: "Error",
    hint: "The class of the error the trace ended on, never its message: TimeoutError, RateLimitError.",
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
    key: TraceColumn.Retrievals,
    label: "Retrievals",
    numeric: true,
    group: "counts",
    hint: "The trace's searches of a vector store or a memory.",
  },
  {
    key: TraceColumn.Errors,
    label: "Errors",
    sort: TraceSort.Errors,
    first: SortDirection.Desc,
    numeric: true,
    group: "counts",
    hint: "How many of the trace's spans failed.",
  },
  {
    key: TraceColumn.Spans,
    label: "Spans",
    numeric: true,
    group: "counts",
    hint: "Every span the trace sent: its own, its steps, its calls and any other.",
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
    hint: "Input tokens the trace's model calls read from a provider's prompt cache.",
  },
  {
    key: TraceColumn.CacheWriteTokens,
    label: "Cache write",
    numeric: true,
    group: "tokens",
    hint: "Input tokens the trace's model calls wrote to a provider's prompt cache.",
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
    key: TraceColumn.Spend,
    label: "Spend",
    sort: TraceSort.Spend,
    first: SortDirection.Desc,
    numeric: true,
    hint: "What the trace's model calls cost: each as it reported, else its tokens at the project's price or the price table's.",
  },
  {
    key: TraceColumn.Duration,
    label: "Duration",
    sort: TraceSort.Duration,
    first: SortDirection.Desc,
    numeric: true,
    hint: "From the trace's first span's start to its last span's end.",
  },
  {
    key: TraceColumn.TimeToFirstToken,
    label: "Time to first token",
    sort: TraceSort.TimeToFirstToken,
    first: SortDirection.Desc,
    numeric: true,
    hint: "How soon its first model call that reported one began to answer.",
  },
  { key: TraceColumn.Models, label: "Models" },
  { key: TraceColumn.Tools, label: "Tools" },
  {
    key: TraceColumn.Tags,
    label: "Tags",
    byTag: true,
    hint: "The trace's tags, each key:value. The heading sorts the list by one key's value, the traces without it last.",
  },
  {
    key: TraceColumn.Scores,
    label: "Scores",
    hint: "The trace's own scores, each name's mean, or a category's values: from your code or a member.",
  },
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

/** A ready-made query a toolbar button applies; a cost preset waits for the list to read spend. */
export type Preset = {
  id: "errors" | "slow" | "tokens";
  label: string;
  hint: string;
};

/** A list's scaffold: its path, table and sorts, an id sort where the id
 *  sorts, the server's default, its ready-made queries and the sorts two of
 *  them take, and how it reads a tag's sort where a tag sorts it. */
export function listSpec<K extends string, S extends string>(spec: {
  path: `/${ListName}`;
  table: ListTable<K, S>;
  sorts: Record<string, S>;
  idSort?: Column<S>;
  defaultSort: S;
  presets: readonly Preset[];
  presetSorts: { slow: S; tokens: S };
  readSort?: (text: string) => S | undefined;
}) {
  const sorts: readonly Column<S>[] = [
    ...(spec.idSort ? [spec.idSort] : []),
    ...sortsOf(spec.table.columns),
  ];
  const parse = (searchParams: SearchParams): ListQuery<S> =>
    parseListQuery(searchParams, spec.sorts, spec.readSort);
  const href = (
    organization: string,
    project: string,
    query: ListQuery<S>,
    patch: Partial<ListQuery<S>> = {},
  ): string => listHref(organization, project, spec.path, query, patch);
  const current = (query: ListQuery<S>): { sort: S; dir: SortDirection } =>
    sortInForce(sorts, spec.defaultSort, query);
  const presetActive = (query: ListQuery<S>, preset: Preset["id"]): boolean => {
    const { sort, dir } = current(query);
    switch (preset) {
      case "errors":
        return query.status?.length === 1 && query.status[0] === SpanStatus.Error;
      case "slow":
      case "tokens":
        return sort === spec.presetSorts[preset] && dir === SortDirection.Desc;
    }
  };
  return {
    table: spec.table,
    presets: spec.presets,
    /** The columns menu, in its rows. */
    menu: columnMenu(spec.table.columns),
    /** `columns` as the table shows them. */
    ordered: (columns: readonly string[]): K[] => ordered(spec.table.columns, columns),
    /** The query the URL carries, every value the server would refuse dropped. */
    parse,
    /** The list's path with `query` in its URL: `patch` changes some keys, and
     *  a key patched to `undefined` is dropped. */
    href,
    /** The sort in force: the URL's, else the server's default. */
    current,
    /** What clicking a column heading asks for, from the first page. */
    next: (query: ListQuery<S>, column: S): { sort: S; dir: SortDirection; page: undefined } => ({
      ...sortAfterClick(sorts, spec.defaultSort, query, column),
      page: undefined,
    }),
    /** Whether the list shows what `preset` asks for. */
    presetActive,
    /** The query with `preset` applied, or taken off where it was: the errors
     *  filter cleared, or the sort set back to the list's default. */
    presetToggled: (query: ListQuery<S>, preset: Preset["id"]): ListQuery<S> => {
      const on = presetActive(query, preset);
      if (preset === "errors") {
        return compact({ ...query, status: on ? undefined : [SpanStatus.Error], page: undefined });
      }
      return compact({
        ...query,
        sort: on ? undefined : spec.presetSorts[preset],
        dir: on ? undefined : SortDirection.Desc,
        page: undefined,
      });
    },
  };
}

/** What `listSpec` builds for a list of columns `K` and sorts `S`. */
export type ListSpec<K extends string, S extends string> = ReturnType<typeof listSpec<K, S>>;

/** `columns` as the trace table shows them. */
export function orderedColumns(columns: readonly string[]): TraceColumn[] {
  return TRACES.ordered(columns);
}

/** The columns the list shows until a person picks others: what a trace was
 *  and what it took, and no id stored as a hash. */
export const DEFAULT_COLUMNS: readonly TraceColumn[] = [
  TraceColumn.Started,
  TraceColumn.Agent,
  TraceColumn.Status,
  TraceColumn.Steps,
  TraceColumn.Tokens,
  TraceColumn.Spend,
  TraceColumn.Duration,
  TraceColumn.Scores,
];

/** The trace list's columns menu. */
export const TRACE_TABLE: ListTable<TraceColumn, TraceSort> = {
  id: TRACE_ID_COLUMN,
  columns: TRACE_COLUMNS,
  defaults: DEFAULT_COLUMNS,
};

/** The trace list, a tag's among its sorts, the server's default newest
 *  first. */
export const TRACES = listSpec<TraceColumn, TraceSort | TagSort>({
  path: "/traces",
  table: TRACE_TABLE,
  sorts: TraceSort,
  defaultSort: TraceSort.Started,
  presets: [
    { id: "errors", label: "Errors", hint: "Only the traces that ended in error" },
    { id: "slow", label: "Slow", hint: "The longest traces first" },
    { id: "tokens", label: "Tokens", hint: "The traces that used the most tokens first" },
  ],
  presetSorts: { slow: TraceSort.Duration, tokens: TraceSort.Tokens },
  readSort: readTagSort,
});

export const currentSort = TRACES.current;
export const nextSort = TRACES.next;

export const presetActive = TRACES.presetActive;
export const presetToggled = TRACES.presetToggled;

/** How many pages the list's total fills at `limit` rows a page: one at
 *  least, so an empty list is on its only page. */
export function pageCount(total: number, limit: number): number {
  return Math.max(1, Math.min(MAX_PAGE, Math.ceil(total / Math.max(1, limit))));
}

/** Where each list's download is served. */
const DOWNLOAD_PATH: Record<ListName, string> = {
  traces: "/traces/export",
  sessions: "/sessions/export",
  users: "/users/export",
};

/** The trace list as a file, through the console's own route: its filters
 *  and sort. */
export function downloadHref(
  organization: string,
  project: string,
  query: TraceQuery,
  format: DownloadFormat,
): string {
  return listDownloadHref("traces", organization, project, query, format);
}

/** `list` as a file, through the console's own route: its filters and sort. */
export function listDownloadHref(
  list: ListName,
  organization: string,
  project: string,
  query: ListQuery<string>,
  format: DownloadFormat,
): string {
  return listHref(organization, project, DOWNLOAD_PATH[list], {
    ...filtersIn(query),
    // The score narrows the file as it narrows the list, or the file is wider than its menu says.
    score: query.score,
    score_min: query.score_min,
    score_max: query.score_max,
    sort: query.sort,
    dir: query.dir,
    days: query.days,
    format,
  });
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

/** A count and what it counts, one or many: `1 trace`, `12,345 traces`. */
export function counted(n: number, one: string, many: string): string {
  return `${formatCount(n)} ${n === 1 ? one : many}`;
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

const SPEED = new Intl.NumberFormat("en-US", { maximumSignificantDigits: 3 });
const SCIENTIFIC_SPEED = new Intl.NumberFormat("en-US", { notation: "scientific", maximumSignificantDigits: 3 });

/** A measured output rate, retaining small positive fractions. */
export function formatOutputSpeed(speed: number): string {
  const format = (speed > 0 && speed < 0.000001) || speed >= 1e9 ? SCIENTIFIC_SPEED : SPEED;
  return `${format.format(speed)} tokens/s`;
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

/** What each list's score filter keeps, said under it: whose score counts. */
export const SCORE_FILTER_HINT: Record<ListName, string> = {
  traces: "Rows with a score of this name, any of its scores within the range given; a yes is 1 and a no 0.",
  sessions:
    "Sessions with a score of this name, their own or one of their traces', any within the range given; a yes is 1 and a no 0.",
  users:
    "Users with a score of this name on one of their traces or sessions, any within the range given; a yes is 1 and a no 0.",
};

/** The word the console gives each status. */
export const STATUS_LABEL: Record<SpanStatus, string> = {
  [SpanStatus.Ok]: "OK",
  [SpanStatus.Error]: "Error",
  [SpanStatus.Cancelled]: "Cancelled",
};

/** The word the console gives each kind of span. */
export const KIND_LABEL: Record<SpanKind, string> = {
  [SpanKind.Agent]: "Agent",
  [SpanKind.TraceStart]: "Trace opened",
  [SpanKind.Step]: "Step",
  [SpanKind.ModelCall]: "Model call",
  [SpanKind.ToolCall]: "Tool call",
  [SpanKind.Retrieval]: "Retrieval",
  [SpanKind.Span]: "Span",
};

/** The word the console gives each kind of change. */
const CHANGE_LABEL: Record<ChangeKind, string> = {
  [ChangeKind.Release]: "release",
  [ChangeKind.Model]: "model",
  [ChangeKind.Prompt]: "prompt",
  [ChangeKind.Tool]: "first call of",
};

/** What a change changed to, and where, its agent left out:
 *  `release 1.4.2 in production`, `prompt triage version 3`; a capped one
 *  saying its day marked no more of its kind, so it may not be the latest. */
export function changeWords(change: Change): string {
  const what =
    change.kind === ChangeKind.Prompt
      ? `${CHANGE_LABEL[change.kind]} ${change.prompt} version ${change.value}`
      : `${CHANGE_LABEL[change.kind]} ${change.value}`;
  const where = change.environment ? `${what} in ${change.environment}` : what;
  const later = change.kind === ChangeKind.Tool ? "first calls" : `${CHANGE_LABEL[change.kind]} changes`;
  return change.capped
    ? `${where}; its later ${later} that UTC day are not marked`
    : where;
}

/** The changes the list's charts mark on one bar, by its start as the server
 *  sent it: a line naming each. */
export type ChangeMark = { at: string; changes: string[] };

/** Each change on the bar it fell in, read off the bars' own starts: the last
 *  to start at or before it, so one before the first is on the first and one
 *  after the last on the last, as the histogram clamps traces. A bar's changes
 *  are one mark, in the order given. */
export function marksOn(bars: readonly { at: string }[], changes: readonly Change[]): ChangeMark[] {
  const starts = bars.map((bar) => Date.parse(bar.at));
  const held = new Map<number, string[]>();
  for (const change of changes) {
    const at = Date.parse(change.changed_at);
    const bar = Math.max(0, starts.findLastIndex((start) => start <= at));
    const lines = held.get(bar) ?? [];
    lines.push(`${change.agent}: ${changeWords(change)}`);
    held.set(bar, lines);
  }
  return bars.flatMap((bar, i) => {
    const lines = held.get(i);
    return lines ? [{ at: bar.at, changes: lines }] : [];
  });
}

/** A count's figures alone: no sign, point, exponent or space. */
const WHOLE = /^\d+$/;

/** A count an attribute holds as text: a whole number from 0 within the safe
 *  integers, else none, so one absent or malformed is never drawn as 0. */
export function countOf(text: string | undefined): number | null {
  if (text === undefined || !WHOLE.test(text)) return null;
  const count = Number(text);
  return Number.isSafeInteger(count) ? count : null;
}

/** What a retrieval asked for and what came back, as sent, never cut to what
 *  it asked for: a memory search counts its records where ours counts documents. */
export function retrievalCounts(attributes: Readonly<Record<string, string>>): {
  asked: number | null;
  returned: number | null;
} {
  return {
    asked: countOf(attributes["gen_ai.retrieval.top_k"]),
    returned: countOf(attributes["telmoni.retrieval.returned"] ?? attributes["gen_ai.memory.record.count"]),
  };
}

/** A retrieval's count as its row says it, `3 of 5`, or `3` where it asked for
 *  no number; none where nothing counted what came back. */
export function retrievalWords(attributes: Readonly<Record<string, string>>): string | null {
  const { asked, returned } = retrievalCounts(attributes);
  if (returned === null) return null;
  return asked === null ? formatCount(returned) : `${formatCount(returned)} of ${formatCount(asked)}`;
}

/** What a retrieval searched: the id its row is named by, else the one it
 *  sent where that ran past a name's cap; none where it sent neither. */
export function retrievalSource(span: {
  name: string;
  attributes: Readonly<Record<string, string>>;
}): string | null {
  const ids = [span.attributes["gen_ai.data_source.id"], span.attributes["gen_ai.memory.store.id"]].filter(
    (id): id is string => id !== undefined && id !== "",
  );
  return ids.find((id) => id === span.name) ?? ids[0] ?? null;
}

/** An id as a row shows it — a trace's, or a session's or a user's as stored:
 *  its first eight characters, enough to tell two apart, the whole behind it. */
export function shortId(id: string): string {
  return id.slice(0, 8);
}

/** The milliseconds between two of the server's times. */
export function spanMillis(span: Pick<TraceSpan, "started_at" | "ended_at">): number {
  return Math.max(0, new Date(span.ended_at).getTime() - new Date(span.started_at).getTime());
}

/** A span's bar on the trace's timeline, as `timelineBars` draws them. */
export type TimelineBar = ReturnType<typeof timelineBars>[number];

/** A trace's bars by their span, for the rows to find theirs. */
export function barsById(spans: TraceSpan[]): Map<string, TimelineBar> {
  return new Map(timelineBars(spans).map((bar) => [bar.span.span_id, bar]));
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
