import type { TraceDetail } from "@/lib/server/entities/traces";
import { spanKey } from "@/lib/trace-graph";
import { spanTree, summarizeTrace, treeRows, type SpanNode } from "@/lib/trace-tree";

// Two runs side by side (`/{organization}/{project}/traces/compare`): their
// figures against each other, and their spans in the order the trace page
// reads them, row by row, where they part marked — the first row whose span
// differs by its kind or what a layout calls it (`spanKey`, as the graph
// tells nodes apart). One walk down both, never an alignment of every pair.

/** The most rows the comparison draws: past them each trace's page has the rest. */
export const COMPARE_ROWS_MAX = 400;

export type ComparedRow = { a: SpanNode | null; b: SpanNode | null };

export type Comparison = {
  rows: ComparedRow[];
  /** The first row where the runs part, and each run's span there; `null`
   *  where every row read is alike. Found over every span, drawn or not. */
  parted: ({ at: number } & ComparedRow) | null;
  /** Whether either run holds more spans than the server sent, so what is
   *  found holds only as far as both were sent. */
  partial: boolean;
  /** Rows past `COMPARE_ROWS_MAX`, not drawn. */
  hidden: number;
};

type Compared = Pick<TraceDetail, "spans" | "truncated">;

/** A trace's spans in the order its page's tree reads them, every branch open. */
function readOrder(trace: Compared): SpanNode[] {
  return treeRows(spanTree(trace.spans), new Set());
}

export function compareRuns(a: Compared, b: Compared): Comparison {
  const left = readOrder(a);
  const right = readOrder(b);
  const length = Math.max(left.length, right.length);
  let parted: Comparison["parted"] = null;
  for (let at = 0; at < length && parted === null; at++) {
    const x = left[at] ?? null;
    const y = right[at] ?? null;
    if (!x || !y || spanKey(x.span) !== spanKey(y.span)) parted = { at, a: x, b: y };
  }
  // A run that ends where the server stopped sending it has not ended there.
  const cut = parted !== null && ((!parted.a && a.truncated) || (!parted.b && b.truncated));
  const shown = Math.min(length, COMPARE_ROWS_MAX);
  return {
    rows: Array.from({ length: shown }, (_, i) => ({ a: left[i] ?? null, b: right[i] ?? null })),
    parted: cut ? null : parted,
    partial: a.truncated || b.truncated,
    hidden: length - shown,
  };
}

/** What a run is measured by, side by side. */
export type RunFigures = {
  durationMs: number;
  inputTokens: number;
  outputTokens: number;
  /** In US dollars; `null` where its spend could not be read. */
  cost: number | null;
  steps: number;
  modelCalls: number;
  toolCalls: number;
};

export function figuresOf(trace: Pick<TraceDetail, "spans" | "spend">): RunFigures {
  const summary = summarizeTrace(trace.spans);
  return {
    durationMs: summary.durationMs,
    inputTokens: summary.inputTokens,
    outputTokens: summary.outputTokens,
    cost: trace.spend?.spent ?? null,
    steps: summary.steps,
    modelCalls: summary.modelCalls,
    toolCalls: summary.toolCalls,
  };
}
