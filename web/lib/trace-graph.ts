import { spanTitle } from "@/lib/layout";
import type { TraceSpan } from "@/lib/server/entities/traces";
import type { SpanNode } from "@/lib/trace-tree";
import { SpanStatus, type SpanKind } from "@/lib/types/enums";

// A run drawn as its agent's graph: each step, model and tool once, by its
// name and kind, each edge what ran next, under its parent or after its
// sibling, and a stretch of siblings that repeats drawn once with how many
// times it went round. Built from the tree a layout shows (`visibleTree`),
// so the graph and the tree never disagree on what a trace holds.

/** A span as the graph and a comparison of two runs tell it from another:
 *  its kind and what a layout calls it. */
export function spanKey(span: Pick<TraceSpan, "kind" | "name" | "model" | "agent">): string {
  return `${span.kind}\u0000${spanTitle(span)}`;
}

/** The most nodes a graph draws: a run of thousands of distinct names is read
 *  in its tree. */
export const GRAPH_NODES_MAX = 60;

/** The longest stretch of siblings a loop is found in: plan, act, check. */
export const LOOP_PERIOD_MAX = 4;

export type GraphNode = {
  key: string;
  kind: SpanKind;
  title: string;
  /** Its spans' ids, in the order they ran. */
  spans: string[];
  failed: number;
  /** How many times it went round where it repeated alone, back to back;
   *  0 where it never did. */
  loop: number;
  /** Its row, from 0 at the top, and its place in the row. */
  rank: number;
  column: number;
};

export type GraphEdge = {
  from: string;
  to: string;
  /** How many times it was taken. */
  count: number;
  /** Whether it runs back up the graph, to a node first seen earlier. */
  back: boolean;
  /** Back to where a loop of several nodes began: how many times it went
   *  round; 0 for any other edge. */
  round: number;
};

export type AgentGraph = {
  nodes: GraphNode[];
  edges: GraphEdge[];
  /** Nodes past `GRAPH_NODES_MAX`, not drawn. */
  hidden: number;
  ranks: number;
  /** The most nodes in one row. */
  widest: number;
};

/** A stretch of a sibling list: `period` keys from `start`, `times` over. */
type Block = { start: number; period: number; times: number };

/** A sibling list cut into its repeats, each as long as it covers the most,
 *  a shorter stretch taken on a tie: A A A A is A four times, not A A twice. */
export function repeats(keys: readonly string[]): Block[] {
  const same = (a: number, b: number, period: number) => {
    for (let i = 0; i < period; i++) if (keys[a + i] !== keys[b + i]) return false;
    return true;
  };
  const blocks: Block[] = [];
  let at = 0;
  while (at < keys.length) {
    let best: Block = { start: at, period: 1, times: 1 };
    for (let period = 1; period <= LOOP_PERIOD_MAX && at + 2 * period <= keys.length; period++) {
      let times = 1;
      while (at + (times + 1) * period <= keys.length && same(at, at + times * period, period)) times++;
      if (times >= 2 && times * period > best.times * best.period) best = { start: at, period, times };
    }
    blocks.push(best);
    at += best.period * best.times;
  }
  return blocks;
}

/** The graph of a trace's tree as a layout shows it. */
export function agentGraph(tree: readonly SpanNode[]): AgentGraph {
  const nodes = new Map<string, GraphNode>();
  const edges = new Map<string, GraphEdge>();
  const edge = (from: string, to: string, count: number, round = 0) => {
    const id = `${from}\u0001${to}`;
    const was = edges.get(id);
    if (was) {
      was.count += count;
      was.round = Math.max(was.round, round);
    } else {
      edges.set(id, { from, to, count, back: false, round });
    }
  };
  const place = (node: SpanNode): string => {
    const key = spanKey(node.span);
    let drawn = nodes.get(key);
    if (!drawn) {
      drawn = {
        key,
        kind: node.span.kind,
        title: spanTitle(node.span),
        spans: [],
        failed: 0,
        loop: 0,
        rank: 0,
        column: 0,
      };
      nodes.set(key, drawn);
    }
    drawn.spans.push(node.span.span_id);
    if (node.span.status === SpanStatus.Error) drawn.failed++;
    return key;
  };

  // Depth first, so a node's place in `nodes` is where it first ran.
  const walk = (list: readonly SpanNode[], parent: string | null) => {
    const keys = list.map((node) => {
      const key = place(node);
      walk(node.children, key);
      return key;
    });
    let prev = parent;
    for (const { start, period, times } of repeats(keys)) {
      const loop = keys.slice(start, start + period);
      const first = loop[0]!;
      const last = loop[period - 1]!;
      if (prev !== null) edge(prev, first, 1);
      for (let i = 1; i < period; i++) edge(loop[i - 1]!, loop[i]!, times);
      if (times > 1) {
        if (period === 1) {
          const node = nodes.get(first)!;
          node.loop = Math.max(node.loop, times);
        } else {
          edge(last, first, times - 1, times);
        }
      }
      prev = last;
    }
  };
  walk(tree, null);

  const order = [...nodes.keys()];
  const drawn = new Set(order.slice(0, GRAPH_NODES_MAX));
  const seen = new Map(order.map((key, i) => [key, i]));
  const kept = [...edges.values()].filter(
    (e) => drawn.has(e.from) && drawn.has(e.to) && e.from !== e.to,
  );
  // Every edge forward runs to a node first seen later, so the order nodes
  // were seen in is one their rows can be settled in, longest path first.
  for (const e of kept) e.back = seen.get(e.to)! < seen.get(e.from)!;
  const out = new Map<string, GraphEdge[]>();
  for (const e of kept) if (!e.back) out.set(e.from, [...(out.get(e.from) ?? []), e]);
  const shown = order.filter((key) => drawn.has(key)).map((key) => nodes.get(key)!);
  for (const node of shown) {
    for (const e of out.get(node.key) ?? []) {
      const to = nodes.get(e.to)!;
      to.rank = Math.max(to.rank, node.rank + 1);
    }
  }
  const rows = new Map<number, number>();
  for (const node of shown) {
    node.column = rows.get(node.rank) ?? 0;
    rows.set(node.rank, node.column + 1);
  }
  return {
    nodes: shown,
    edges: kept,
    hidden: order.length - shown.length,
    ranks: rows.size === 0 ? 0 : Math.max(...rows.keys()) + 1,
    widest: rows.size === 0 ? 0 : Math.max(...rows.values()),
  };
}
