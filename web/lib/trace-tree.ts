import type { TraceSpan } from "@/lib/server/entities/traces";
import { KIND_LABEL } from "@/lib/traces";
import { SpanKind } from "@/lib/types/enums";

// One trace as the tree its spans were recorded in, for the view that reads
// it a span at a time (`components/traces/trace-view.tsx`): each span under
// its parent.

/** A span and the spans recorded under it. */
export type SpanNode = {
  span: TraceSpan;
  /** How deep it stands: 0 at the top. */
  depth: number;
  /** Where it stands among the spans under the same parent, from 1, and how
   *  many those are: a tree drawn as one flat list says both outright, or a
   *  screen reader counts every row as one set. */
  position: number;
  siblings: number;
  children: SpanNode[];
};

/** A trace's spans as the tree they were recorded in, siblings in the order
 *  they started, the span's id settling a tie so two renders agree. A span
 *  whose parent is not among them — the run's own, or one under a span
 *  sampled out or still on its way — stands at the top, as does one caught
 *  in a cycle, which only a hand-written export can draw: it would otherwise
 *  be under nothing and drawn nowhere. */
export function spanTree(spans: readonly TraceSpan[]): SpanNode[] {
  // Each start read once, not at every comparison: a run holds thousands.
  const start = new Map(spans.map((s) => [s.span_id, Date.parse(s.started_at)]));
  const byStart = (a: TraceSpan, b: TraceSpan) =>
    start.get(a.span_id)! - start.get(b.span_id)! ||
    (a.span_id < b.span_id ? -1 : a.span_id > b.span_id ? 1 : 0);

  const under = new Map<string, TraceSpan[]>();
  const tops: TraceSpan[] = [];
  for (const span of spans) {
    const parent = span.parent_span_id;
    if (parent && parent !== span.span_id && start.has(parent)) {
      const children = under.get(parent);
      if (children) children.push(span);
      else under.set(parent, [span]);
    } else {
      tops.push(span);
    }
  }

  const placed = new Set<string>();
  const grow = (span: TraceSpan, depth: number): SpanNode => {
    placed.add(span.span_id);
    const children = (under.get(span.span_id) ?? [])
      .filter((child) => !placed.has(child.span_id))
      .sort(byStart)
      .map((child) => grow(child, depth + 1));
    return { span, depth, position: 0, siblings: 0, children };
  };
  const nodes = tops.sort(byStart).map((span) => grow(span, 0));
  for (const span of [...spans].sort(byStart)) {
    if (!placed.has(span.span_id)) nodes.push(grow(span, 0));
  }
  nodes.sort((a, b) => byStart(a.span, b.span));

  const count = (list: SpanNode[]) =>
    list.forEach((node, i) => {
      node.position = i + 1;
      node.siblings = list.length;
      count(node.children);
    });
  count(nodes);
  return nodes;
}

/** The rows a tree shows, depth first: every node but those under a shut
 *  one. */
export function treeRows(nodes: readonly SpanNode[], shut: ReadonlySet<string>): SpanNode[] {
  const rows: SpanNode[] = [];
  const walk = (list: readonly SpanNode[]) => {
    for (const node of list) {
      rows.push(node);
      if (!shut.has(node.span.span_id)) walk(node.children);
    }
  };
  walk(nodes);
  return rows;
}

/** What a search finds a span by: its name, its kind, the model and the
 *  provider it called, and the class of error it ended on. */
function searchText(span: TraceSpan): string {
  return [span.name, KIND_LABEL[span.kind], span.model, span.provider, span.error_type]
    .join(" ")
    .toLowerCase();
}

/** The rows a search leaves: each span it finds with the spans above it, so a
 *  match is read where it ran, whatever was shut. */
export function matchingRows(nodes: readonly SpanNode[], search: string): SpanNode[] {
  const needle = search.trim().toLowerCase();
  const rows: SpanNode[] = [];
  const walk = (list: readonly SpanNode[]): boolean => {
    let kept = false;
    for (const node of list) {
      const at = rows.length;
      rows.push(node);
      const below = walk(node.children);
      if (below || searchText(node.span).includes(needle)) kept = true;
      else rows.length = at;
    }
    return kept;
  };
  walk(nodes);
  return rows;
}

/** Every branch at `depth` or deeper: what a large trace opens with shut. */
export function branchesFrom(nodes: readonly SpanNode[], depth: number): Set<string> {
  const branches = new Set<string>();
  const walk = (list: readonly SpanNode[]) => {
    for (const node of list) {
      if (node.depth >= depth && node.children.length > 0) branches.add(node.span.span_id);
      walk(node.children);
    }
  };
  walk(nodes);
  return branches;
}

/** Whether the span `id` stands somewhere under `ancestor`, by the parents
 *  the spans name; a cycle ends the walk. */
export function isUnder(
  parentOf: ReadonlyMap<string, string>,
  id: string,
  ancestor: string,
): boolean {
  const seen = new Set<string>();
  let at = parentOf.get(id);
  while (at && !seen.has(at)) {
    if (at === ancestor) return true;
    seen.add(at);
    at = parentOf.get(at);
  }
  return false;
}

/** `set` with `id` taken out where it is in, else put in. */
export function toggled<T>(set: ReadonlySet<T>, id: T): Set<T> {
  const next = new Set(set);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/** The tree as the view holds it: the span chosen, the branches shut, and
 *  each span's parent, by which a branch shutting moves the chosen span out
 *  of it. */
export type TreeState = {
  chosen: string;
  shut: ReadonlySet<string>;
  parentOf: ReadonlyMap<string, string>;
};

export type TreeAction =
  | { type: "choose"; id: string }
  | { type: "toggle"; id: string }
  /** The spans drawn moved on, as a run still going is read again. */
  | { type: "spans"; parentOf: ReadonlyMap<string, string> };

/** One transition for the one `dispatch` every row is handed. Shutting the
 *  branch the chosen span sits in chooses the branch, as a file tree does, so
 *  what stands chosen stays in sight. */
export function treeReducer(state: TreeState, action: TreeAction): TreeState {
  if (action.type === "choose") {
    return state.chosen === action.id ? state : { ...state, chosen: action.id };
  }
  if (action.type === "spans") return { ...state, parentOf: action.parentOf };
  const shut = toggled(state.shut, action.id);
  const shutting = shut.has(action.id);
  const chosen =
    shutting && isUnder(state.parentOf, state.chosen, action.id) ? action.id : state.chosen;
  return { ...state, chosen, shut };
}

/** The node above `node` in `rows`, the way ← walks up a tree: the nearest
 *  row before it one level up. */
export function parentRow(rows: readonly SpanNode[], node: SpanNode): SpanNode | null {
  const at = rows.indexOf(node);
  for (let i = at - 1; i >= 0; i--) {
    if (rows[i]!.depth < node.depth) return rows[i]!;
  }
  return null;
}

/** A trace summed from its spans alone, as its header reads it. */
export type TraceSummary = {
  /** The run's own span, or `null` while it has not arrived. */
  run: TraceSpan | null;
  /** What the trace is named by: the run's span, or the first span while
   *  the run is still going. */
  root: TraceSpan;
  startedAt: string;
  endedAt: string;
  durationMs: number;
  steps: number;
  modelCalls: number;
  toolCalls: number;
  inputTokens: number;
  outputTokens: number;
};

/** The trace's sums. The tokens are the model calls' alone, as the list sums
 *  them: a span of another kind may repeat its calls' usage, and would count
 *  it twice. `spans` holds one span at least, as a trace the server answers
 *  does. */
export function summarizeTrace(spans: readonly TraceSpan[]): TraceSummary {
  const first = spans[0]!;
  const run = spans.find((s) => s.kind === SpanKind.Run) ?? null;
  // As times, never as text: the server writes as few fractional digits as a
  // time needs, and `…:00Z` sorts after `…:00.250Z` as text.
  const startedAt = spans.reduce(
    (earliest, s) => (Date.parse(s.started_at) < Date.parse(earliest) ? s.started_at : earliest),
    first.started_at,
  );
  const endedAt = spans.reduce(
    (latest, s) => (Date.parse(s.ended_at) > Date.parse(latest) ? s.ended_at : latest),
    first.ended_at,
  );
  const modelCalls = spans.filter((s) => s.kind === SpanKind.ModelCall);
  return {
    run,
    root: run ?? first,
    startedAt,
    endedAt,
    durationMs: Math.max(0, Date.parse(endedAt) - Date.parse(startedAt)),
    steps: spans.filter((s) => s.kind === SpanKind.Step).length,
    modelCalls: modelCalls.length,
    toolCalls: spans.filter((s) => s.kind === SpanKind.ToolCall).length,
    inputTokens: modelCalls.reduce((sum, s) => sum + s.input_tokens, 0),
    outputTokens: modelCalls.reduce((sum, s) => sum + s.output_tokens, 0),
  };
}
