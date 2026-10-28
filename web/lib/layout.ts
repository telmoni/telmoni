import { z } from "zod";

import type { PinnedSpan } from "@/lib/server/entities/sessions";
import type { LayoutQuery, TraceLayout, TraceSpan } from "@/lib/server/entities/traces";
import type { SearchParams } from "@/lib/list-query";
import { one } from "@/lib/list-query";
import { isOneOf } from "@/lib/one-of";
import type { SpanNode } from "@/lib/trace-tree";
import { orderedColumns } from "@/lib/traces";
import { LayoutDepth, LayoutOpen, SpanKind, SpanStatus, TraceColumn } from "@/lib/types/enums";

// A session's page and a trace's open in one layout of one set of options,
// and any option can be changed from Customize: each trace's line alone to
// begin with, as a chatbot's reviewer reads a conversation, and its steps
// and calls as nested work once opened, as an agent harness's builder reads
// them, so a team's own cut is never a third thing. A link holds a saved
// layout (`?view=`); else the person's last choice, kept in their browser
// (`lib/trace-prefs.ts`); else the project's default. What this module does
// is name the default, read a layout from wherever it comes, and say what a
// layout shows of a trace's spans.

/** The calls a layout shows, any value of each list admitting a call, and
 *  an empty list every call. */
export type LayoutFilter = {
  kind: readonly SpanKind[];
  status: readonly SpanStatus[];
  name: readonly string[];
};

/** The one set of options every layout is a cut of. */
export type LayoutOptions = {
  /** How far a trace opens. */
  depth: LayoutDepth;
  /** Which traces, and which branches of them, start open. */
  open: LayoutOpen;
  /** Each span's bar on its trace's own line of time. */
  timeline: boolean;
  /** The trace list's columns a trace shows. */
  columns: readonly TraceColumn[];
  /** Span names whose spans show under each trace while it is shut. */
  pinned: readonly string[];
  filter: LayoutFilter;
  /** The run's own input and output, once the project keeps content. */
  messages: boolean;
  /** The trace's scores, once scored. */
  scores: boolean;
};

/** The most span names a layout pins, as the server takes them. */
export const PINNED_MAX = 8;

/** The most traces `open: all` opens as a session's page draws: each is a
 *  read of its spans, and a reader's pace is 120 a minute. */
export const OPEN_AT_MOST = 20;

/** Past this many spans a trace opens with its branches shut whatever the
 *  layout says, so a run of thousands draws its first rows at once. */
export const OPEN_ALL_UP_TO = 400;

/** The most pinned spans one trace shows, as the session lane caps them: a
 *  step a loop ran again and again is read in the trace itself. */
export const PINNED_PER_TRACE = 32;

export const NO_FILTER: LayoutFilter = { kind: [], status: [], name: [] };

/** The layout every page opens in until something else is chosen: each
 *  trace's line alone, a turn of a conversation — when it started, the user,
 *  how it ended and on what, its tokens, how long it took, and the models
 *  and tools it called by name, in the list's order — none open, no
 *  timeline, with its messages and scores once each shows. */
export const DEFAULT_LAYOUT: LayoutOptions = {
  depth: LayoutDepth.Trace,
  open: LayoutOpen.None,
  timeline: false,
  columns: [
    TraceColumn.Started,
    TraceColumn.User,
    TraceColumn.Status,
    TraceColumn.ErrorType,
    TraceColumn.Tokens,
    TraceColumn.Duration,
    TraceColumn.Models,
    TraceColumn.Tools,
  ],
  pinned: [],
  filter: NO_FILTER,
  messages: true,
  scores: true,
};

/** What the console calls the default layout where a name is wanted. */
export const DEFAULT_LAYOUT_NAME = "the default layout";

const sameList = (a: readonly string[], b: readonly string[]) =>
  [...a].sort().join("\u0000") === [...b].sort().join("\u0000");

/** Whether two layouts show the same: every option the same, a list's
 *  values in any order. */
export function sameLayout(a: LayoutOptions, b: LayoutOptions): boolean {
  return (
    a.depth === b.depth &&
    a.open === b.open &&
    a.timeline === b.timeline &&
    a.messages === b.messages &&
    a.scores === b.scores &&
    sameList(a.columns, b.columns) &&
    sameList(a.pinned, b.pinned) &&
    sameList(a.filter.kind, b.filter.kind) &&
    sameList(a.filter.status, b.filter.status) &&
    sameList(a.filter.name, b.filter.name)
  );
}

/** A list of names as a layout keeps them: each trimmed and once, up to
 *  `max`. */
function names(given: readonly string[], max: number): string[] {
  const kept: string[] = [];
  for (const text of given) {
    const trimmed = text.trim();
    if (trimmed && !kept.includes(trimmed)) kept.push(trimmed);
    if (kept.length === max) break;
  }
  return kept;
}

/** The options a saved layout holds: its query and its columns, read as one,
 *  each list as the page keeps it. */
export function layoutOf(layout: Pick<TraceLayout, "query" | "columns">): LayoutOptions {
  const { query } = layout;
  return {
    depth: query.depth,
    open: query.open,
    timeline: query.timeline,
    columns: orderedColumns(layout.columns),
    pinned: names(query.pinned ?? [], PINNED_MAX),
    filter: {
      kind: [...new Set(query.filter.kind ?? [])],
      status: [...new Set(query.filter.status ?? [])],
      name: names(query.filter.name ?? [], 50),
    },
    messages: query.messages,
    scores: query.scores,
  };
}

/** What a save sends as a layout's `query`: every option but the columns,
 *  which ride beside it. */
export function layoutQueryOf(options: LayoutOptions): LayoutQuery {
  return {
    depth: options.depth,
    open: options.open,
    timeline: options.timeline,
    pinned: [...options.pinned],
    filter: {
      kind: [...options.filter.kind],
      status: [...options.filter.status],
      name: [...options.filter.name],
    },
    messages: options.messages,
    scores: options.scores,
  };
}

/** A list of names as the browser kept it, whatever else it put among them;
 *  none where it kept no list. Optional before the transform, which alone
 *  would make the key required. */
const Strings = z
  .unknown()
  .optional()
  .transform((v) => (Array.isArray(v) ? v.filter((c): c is string => typeof c === "string") : []));

const kinds = Object.values(SpanKind);
const statuses = Object.values(SpanStatus);

/** What the browser kept of a layout's options: a value this build does not
 *  know — an older spelling, a column since renamed — makes the whole
 *  unreadable, since a layout half-read would show something nobody chose;
 *  a column or a kind it does not know is left out of its list. */
const StoredOptionsSchema = z.object({
  depth: z.enum(LayoutDepth),
  open: z.enum(LayoutOpen),
  timeline: z.boolean(),
  columns: Strings.transform((v) => orderedColumns(v)),
  pinned: Strings.transform((v) => names(v, PINNED_MAX)),
  filter: z.object({
    kind: Strings.transform((v) => [...new Set(v.filter((k): k is SpanKind => isOneOf(kinds, k)))]),
    status: Strings.transform((v) => [
      ...new Set(v.filter((s): s is SpanStatus => isOneOf(statuses, s))),
    ]),
    name: Strings.transform((v) => names(v, 50)),
  }),
  messages: z.boolean(),
  scores: z.boolean(),
});

export function readLayoutOptions(raw: unknown): LayoutOptions | null {
  const parsed = StoredOptionsSchema.safeParse(raw);
  return parsed.success ? parsed.data : null;
}

/** A layout as a person chose it: the saved layout it was opened from, if
 *  one, and the options as they stand now. */
export type LayoutChoice = {
  /** The saved layout the options were opened from, by its id; `null` for
   *  a cut of the default. */
  view: string | null;
  options: LayoutOptions;
};

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** What the browser kept of a choice; `null` for one this build cannot read
 *  whole. A view that names none reads as a cut of the default, and a key
 *  an older build kept beside these is passed over. */
const StoredChoiceSchema = z.object({
  view: z.string().regex(UUID).nullable().catch(null),
  options: StoredOptionsSchema,
});

export function readLayoutChoice(raw: unknown): LayoutChoice | null {
  const parsed = StoredChoiceSchema.safeParse(raw);
  return parsed.success ? parsed.data : null;
}

/** The saved layout a link names in `?view=`: a value that names none is as
 *  if left out. */
export type LayoutUi = { view?: string };

export const VIEW_PARAM = "view";

export function parseLayoutUi(searchParams: SearchParams): LayoutUi {
  const view = one(searchParams[VIEW_PARAM]);
  return view !== undefined && UUID.test(view) ? { view } : {};
}

/** The choice a saved layout opens as it was saved. */
export function choiceOfLayout(layout: TraceLayout): LayoutChoice {
  return { view: layout.id, options: layoutOf(layout) };
}

/** The choice the default opens. */
export function choiceOfDefault(): LayoutChoice {
  return { view: null, options: DEFAULT_LAYOUT };
}

/** Where a page's layout came from, for what says so. */
export type LayoutSource = "link" | "browser" | "project";

/** The layout a page opens in: the link's saved layout first, then the
 *  person's last choice, then the project's default, then the default
 *  itself. A saved layout the project no longer keeps is passed over, and a
 *  kept choice that named one stands as a cut of its own. */
export function resolveLayout(
  url: LayoutUi,
  kept: LayoutChoice | null,
  project: { view_id: string | null } | null,
  layouts: readonly TraceLayout[],
): { choice: LayoutChoice; source: LayoutSource } {
  const saved = (id: string | null | undefined) =>
    id ? layouts.find((layout) => layout.id === id) : undefined;
  const linked = saved(url.view);
  if (linked) return { choice: choiceOfLayout(linked), source: "link" };
  if (kept) {
    const view = saved(kept.view) ? kept.view : null;
    return { choice: { view, options: kept.options }, source: "browser" };
  }
  const own = saved(project?.view_id);
  if (own) return { choice: choiceOfLayout(own), source: "project" };
  return { choice: choiceOfDefault(), source: "project" };
}

/** Whether the options have moved off what the choice began from: the
 *  saved layout's, or the default. */
export function edited(choice: LayoutChoice, layouts: readonly TraceLayout[]): boolean {
  const from = choice.view ? layouts.find((layout) => layout.id === choice.view) : undefined;
  return !sameLayout(choice.options, from ? layoutOf(from) : DEFAULT_LAYOUT);
}

/** The page's query string with the layout's part of it as the choice is:
 *  a saved layout named, or nothing for a cut of one or of the default — a
 *  link then opens on what the person's browser or the project holds. */
export function withLayoutParams(search: string, choice: LayoutChoice, isEdited: boolean): string {
  const params = new URLSearchParams(search);
  params.delete(VIEW_PARAM);
  if (!isEdited && choice.view) params.set(VIEW_PARAM, choice.view);
  const text = params.toString();
  return text ? `?${text}` : "";
}

/** Whether the depth shows a span: none at `trace`; the steps, and a run
 *  inside the run — a subagent's — at `steps`; every span at `calls`. */
export function depthAdmits(depth: LayoutDepth, span: Pick<TraceSpan, "kind">): boolean {
  switch (depth) {
    case LayoutDepth.Trace:
      return false;
    case LayoutDepth.Steps:
      return span.kind === SpanKind.Step || span.kind === SpanKind.Run;
    case LayoutDepth.Calls:
      return true;
  }
}

/** Whether the filter admits a span: each list it names holds the span's
 *  value, and a list left empty holds every value. */
export function filterAdmits(
  filter: LayoutFilter,
  span: Pick<TraceSpan, "kind" | "status" | "name">,
): boolean {
  return (
    (filter.kind.length === 0 || filter.kind.includes(span.kind)) &&
    (filter.status.length === 0 || filter.status.includes(span.status)) &&
    (filter.name.length === 0 || filter.name.includes(span.name))
  );
}

/** A trace's tree as the layout shows it: the run's own span at the top,
 *  and under it only the spans the depth and the filter admit, a span hidden
 *  taking the spans under it with it, as a branch shut does. */
export function visibleTree(nodes: readonly SpanNode[], options: LayoutOptions): SpanNode[] {
  const admits = (node: SpanNode) =>
    node.depth === 0 ||
    (depthAdmits(options.depth, node.span) && filterAdmits(options.filter, node.span));
  const prune = (list: readonly SpanNode[]): SpanNode[] =>
    list
      .filter(admits)
      .map((node) => ({ ...node, children: prune(node.children) }));
  const pruned = prune(nodes);
  const count = (list: SpanNode[]) =>
    list.forEach((node, i) => {
      node.position = i + 1;
      node.siblings = list.length;
      count(node.children);
    });
  count(pruned);
  return pruned;
}

/** Whether a span, or one under it, failed. */
export function holdsFailure(node: SpanNode): boolean {
  return node.span.status === SpanStatus.Error || node.children.some(holdsFailure);
}

/** How a trace of `spans` spans opens under the layout: as the layout says,
 *  until the trace is too large to draw open. */
export function openingFor(spans: number, open: LayoutOpen): LayoutOpen {
  return spans > OPEN_ALL_UP_TO ? LayoutOpen.None : open;
}

/** The branches a tree starts with shut, as the layout opens them: every
 *  branch at `none`; those with no failure inside at `failed`; none at
 *  `all`. The run's own branch is never shut, since the tree is its spans. */
export function shutBranches(nodes: readonly SpanNode[], open: LayoutOpen): Set<string> {
  const shut = new Set<string>();
  const walk = (list: readonly SpanNode[]) => {
    for (const node of list) {
      if (node.children.length > 0 && node.depth > 0) {
        if (open === LayoutOpen.None || (open === LayoutOpen.Failed && !holdsFailure(node))) {
          shut.add(node.span.span_id);
        }
      }
      walk(node.children);
    }
  };
  walk(nodes);
  return shut;
}

/** Whether a session's trace starts open: none; those that failed, or hold
 *  a span that did; or all of them. A trace a depth of `trace` shows
 *  nothing of stays shut, having nothing to open. */
export function traceOpensByDefault(
  options: Pick<LayoutOptions, "open" | "depth">,
  trace: { status: SpanStatus; errors: number },
): boolean {
  if (options.depth === LayoutDepth.Trace) return false;
  switch (options.open) {
    case LayoutOpen.None:
      return false;
    case LayoutOpen.Failed:
      return trace.status === SpanStatus.Error || trace.errors > 0;
    case LayoutOpen.All:
      return true;
  }
}

/** The traces a session's page opens as it draws: those the layout opens,
 *  the first `OPEN_AT_MOST` of them. */
export function tracesOpenedFirst(
  options: Pick<LayoutOptions, "open" | "depth">,
  traces: readonly { run_id: string; status: SpanStatus; errors: number }[],
): Set<string> {
  const opened = new Set<string>();
  for (const trace of traces) {
    if (opened.size === OPEN_AT_MOST) break;
    if (traceOpensByDefault(options, trace)) opened.add(trace.run_id);
  }
  return opened;
}

/** What a span is called in a layout: a model call by its model, a tool
 *  call by its tool, a run inside the run by its agent, a step by its name. */
export function spanTitle(
  span: Pick<TraceSpan | PinnedSpan, "kind" | "name" | "model"> & { agent?: string },
): string {
  switch (span.kind) {
    case SpanKind.ModelCall:
      return span.model || span.name;
    case SpanKind.Run:
      return span.agent || span.name;
    default:
      return span.name;
  }
}

/** `names` with `name` added, trimmed: as they were where it is empty, there
 *  already, or the list is at `max`. What the Customize dialogs' fields do. */
export function addName(names: readonly string[], name: string, max: number): string[] {
  const trimmed = name.trim();
  if (!trimmed || names.includes(trimmed) || names.length >= max) return [...names];
  return [...names, trimmed];
}
