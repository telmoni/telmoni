import type { TraceQuery, TraceSpan } from "@/lib/server/entities/traces";
import { projectPath } from "@/lib/slug";
import { SortDirection, SpanKind, SpanStatus, TraceSort } from "@/lib/types/enums";

import {
  compact,
  listHref,
  one,
  oneOf,
  parseWindow,
  sortAfterClick,
  sortInForce,
  type Column,
  type SearchParams,
} from "./list-query";

// The trace list's URL is its state: every filter and the sort are query
// parameters, so a link holds them and a page sent to a colleague opens as
// it was left (`list-query.ts`). What this module does is read that URL,
// write it with one thing changed, and put the server's numbers into words.

/** The query the URL carries, every value the server would refuse dropped
 *  — a status that is no status, a day count past the window — so a stale
 *  or hand-edited link still opens the list rather than a 400. */
export function parseTraceQuery(searchParams: SearchParams): TraceQuery {
  return compact({
    agent: one(searchParams.agent),
    user: one(searchParams.user),
    session: one(searchParams.session),
    status: oneOf(searchParams.status, SpanStatus),
    model: one(searchParams.model),
    tool: one(searchParams.tool),
    sort: oneOf(searchParams.sort, TraceSort),
    dir: oneOf(searchParams.dir, SortDirection),
    ...parseWindow(searchParams),
  });
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

/** A trace's own page. */
export function traceHref(organization: string, project: string, runId: string): string {
  return projectPath(organization, project, `/traces/${encodeURIComponent(runId)}`);
}

/** The columns the list sorts by, in the table's order, each with its
 *  heading and the way it sorts when first clicked: the newest, the longest
 *  and the most first; names in the alphabet's order. */
export const TRACE_COLUMNS: readonly Column<TraceSort>[] = [
  { sort: TraceSort.Started, label: "Started", first: SortDirection.Desc },
  { sort: TraceSort.Agent, label: "Agent", first: SortDirection.Asc },
  { sort: TraceSort.User, label: "User", first: SortDirection.Asc },
  { sort: TraceSort.Session, label: "Session", first: SortDirection.Asc },
  { sort: TraceSort.Status, label: "Status", first: SortDirection.Asc },
  { sort: TraceSort.Steps, label: "Steps", first: SortDirection.Desc },
  { sort: TraceSort.Tokens, label: "Tokens", first: SortDirection.Desc },
  { sort: TraceSort.Duration, label: "Duration", first: SortDirection.Desc },
];

/** The sort in force: the URL's, else the server's default, newest first. */
export function currentSort(query: TraceQuery): { sort: TraceSort; dir: SortDirection } {
  return sortInForce(TRACE_COLUMNS, TraceSort.Started, query);
}

/** What clicking a column heading asks for: the other way when the list is
 *  already sorted by it, its own first way otherwise. */
export function nextSort(
  query: TraceQuery,
  column: TraceSort,
): { sort: TraceSort; dir: SortDirection } {
  return sortAfterClick(TRACE_COLUMNS, TraceSort.Started, query, column);
}

/** The filters in force, by the word the page shows for each. */
export function activeFilters(query: TraceQuery): { key: keyof TraceQuery; label: string; value: string }[] {
  const named: [keyof TraceQuery, string][] = [
    ["agent", "Agent"],
    ["user", "User"],
    ["session", "Session"],
    ["status", "Status"],
    ["model", "Model"],
    ["tool", "Tool"],
  ];
  return named.flatMap(([key, label]) => {
    const value = query[key];
    return value === undefined ? [] : [{ key, label, value: String(value) }];
  });
}

const COUNT = new Intl.NumberFormat();

/** A count, grouped the reader's way. */
export function formatCount(n: number): string {
  return COUNT.format(n);
}

/** A duration in milliseconds, in the unit a person reads it in: `340 ms`,
 *  `1.2 s`, `2 min 3 s`, `1 h 4 min`. */
export function formatDuration(ms: number): string {
  if (ms < 0) ms = 0;
  if (ms < 1_000) return `${Math.round(ms)} ms`;
  const seconds = ms / 1_000;
  if (seconds < 60) return `${(Math.round(seconds * 10) / 10).toString()} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    const rest = Math.round(seconds - minutes * 60);
    return rest ? `${minutes} min ${rest} s` : `${minutes} min`;
  }
  const hours = Math.floor(minutes / 60);
  const rest = minutes - hours * 60;
  return rest ? `${hours} h ${rest} min` : `${hours} h`;
}

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

/** A step of a trace with the calls made inside it, in the order they
 *  started: the shape the trace page lists first. */
export type StepGroup = {
  /** The step, or `null` for the calls made straight under the run. */
  step: TraceSpan | null;
  calls: TraceSpan[];
};

/** A trace's spans as steps and their calls. A call nests under the nearest
 *  step above it, whatever lies between; one under no step is the run's own;
 *  a span of no kind the list counts — an exporter's HTTP call — is left to
 *  the timeline. */
export function groupSteps(spans: TraceSpan[]): { run: TraceSpan | null; groups: StepGroup[] } {
  const byId = new Map(spans.map((s) => [s.span_id, s]));
  const run = spans.find((s) => s.kind === SpanKind.Run) ?? null;
  const steps = spans.filter((s) => s.kind === SpanKind.Step);
  const groups = new Map<string | null, StepGroup>();
  groups.set(null, { step: null, calls: [] });
  for (const step of steps) groups.set(step.span_id, { step, calls: [] });

  const nearestStep = (span: TraceSpan): string | null => {
    let parent = byId.get(span.parent_span_id);
    const seen = new Set<string>();
    while (parent && !seen.has(parent.span_id)) {
      if (parent.kind === SpanKind.Step) return parent.span_id;
      seen.add(parent.span_id);
      parent = byId.get(parent.parent_span_id);
    }
    return null;
  };
  for (const span of spans) {
    if (span.kind !== SpanKind.ModelCall && span.kind !== SpanKind.ToolCall) continue;
    groups.get(nearestStep(span))?.calls.push(span);
  }
  const ordered = [...groups.values()].filter((g) => g.step !== null || g.calls.length > 0);
  const startOf = (g: StepGroup) =>
    new Date((g.step ?? g.calls[0]!).started_at).getTime();
  ordered.sort((a, b) => startOf(a) - startOf(b));
  return { run, groups: ordered };
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
