"use client";

import { memo, useId, useMemo } from "react";
import { Repeat } from "lucide-react";

import { KindIcon, TruncatedNote } from "@/components/traces/span-parts";
import type { TraceDetail } from "@/lib/server/entities/traces";
import { agentGraph, type AgentGraph, type GraphNode } from "@/lib/trace-graph";
import type { SpanNode } from "@/lib/trace-tree";
import { KIND_LABEL, formatCount } from "@/lib/traces";
import { SpanStatus } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

// A trace as its agent's graph (`lib/trace-graph.ts`), beside the outline
// (`trace-outline.tsx`) as another way the layout draws one: rows of nodes
// joined top to bottom in the order they ran, a way back up drawn on the
// right, each node a list item that says what it is and what ran after it.

const NODE_W = 168;
const NODE_H = 36;
const GAP_X = 16;
const GAP_Y = 32;
/** Room on the right for the ways back up. */
const BACK_ROOM = 48;

/** What a node says of itself, as its title. */
function nodeWords(node: GraphNode): string {
  const ran = node.spans.length === 1 ? "ran once" : `ran ${formatCount(node.spans.length)} times`;
  const loop = node.loop > 1 ? `, ${formatCount(node.loop)} times in a row` : "";
  const failed = node.failed > 0 ? `, ${formatCount(node.failed)} failed` : "";
  return `${node.title} (${KIND_LABEL[node.kind].toLowerCase()}), ${ran}${loop}${failed}`;
}

/** A node and what ran after it, as a screen reader hears it. */
function heardAs(graph: AgentGraph, byKey: ReadonlyMap<string, GraphNode>, node: GraphNode): string {
  const after = graph.edges
    .filter((e) => e.from === node.key)
    .map((e) => {
      const times =
        e.round > 1 ? `, round ${formatCount(e.round)} times` : e.count > 1 ? `, ${formatCount(e.count)} times` : "";
      return `${e.back ? "back to" : "then"} ${byKey.get(e.to)!.title}${times}`;
    });
  return [nodeWords(node), ...after].join("; ");
}

export const TraceGraph = memo(function TraceGraph({
  trace,
  tree,
  chosen,
  onChoose,
  className,
}: {
  trace: Pick<TraceDetail, "truncated" | "spans">;
  /** The trace's tree as the layout shows it (`visibleTree`). */
  tree: readonly SpanNode[];
  /** The span chosen, whose node stands out. */
  chosen?: string;
  /** Where a node is chosen: its first span, or its next where it holds the
   *  span chosen. Without it the graph is only read. */
  onChoose?: (spanId: string) => void;
  className?: string;
}) {
  const graph = useMemo(() => agentGraph(tree), [tree]);
  // The run's own node alone is nothing drawn, as the outline's empty list is.
  if (graph.nodes.length <= 1 && (graph.nodes[0]?.spans.length ?? 0) <= 1) {
    return (
      <p className={cn("px-2 py-1.5 text-xs text-muted-foreground", className)}>
        {trace.spans.length > 1
          ? "No span of this trace is shown under this layout."
          : "No span under the run yet."}
      </p>
    );
  }
  return (
    <div className={cn("grid gap-2", className)}>
      <GraphDrawing graph={graph} chosen={chosen} onChoose={onChoose} />
      {graph.hidden > 0 && (
        <p className="px-2 text-xs text-muted-foreground">
          {formatCount(graph.hidden)} more {graph.hidden === 1 ? "name" : "names"} not drawn.
        </p>
      )}
      <TruncatedNote trace={trace} className="px-2" />
    </div>
  );
});

/** Where a node stands: its row centred under the widest. */
function at(graph: AgentGraph, node: GraphNode, inRow: number) {
  const offset = ((graph.widest - inRow) * (NODE_W + GAP_X)) / 2;
  return { x: offset + node.column * (NODE_W + GAP_X), y: node.rank * (NODE_H + GAP_Y) };
}

function GraphDrawing({
  graph,
  chosen,
  onChoose,
}: {
  graph: AgentGraph;
  chosen?: string;
  onChoose?: (spanId: string) => void;
}) {
  const arrow = useId();
  const inRow = new Map<number, number>();
  for (const node of graph.nodes) inRow.set(node.rank, (inRow.get(node.rank) ?? 0) + 1);
  const where = new Map(graph.nodes.map((n) => [n.key, at(graph, n, inRow.get(n.rank)!)]));
  const width = graph.widest * (NODE_W + GAP_X) - GAP_X + BACK_ROOM;
  const height = graph.ranks * (NODE_H + GAP_Y) - GAP_Y;
  const byKey = new Map(graph.nodes.map((n) => [n.key, n]));

  const choose = (node: GraphNode) => {
    if (!onChoose) return;
    const i = chosen ? node.spans.indexOf(chosen) : -1;
    onChoose(node.spans[(i + 1) % node.spans.length]!);
  };

  return (
    <div className="overflow-x-auto px-2 pt-1">
      <div role="list" aria-label="The agent's graph" className="relative" style={{ width, height }}>
        <svg aria-hidden="true" width={width} height={height} className="absolute inset-0 text-muted-foreground">
          <defs>
            <marker id={arrow} viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto">
              <path d="M0,0 L8,4 L0,8 z" fill="currentColor" />
            </marker>
          </defs>
          {graph.edges.map((e) => {
            const a = where.get(e.from)!;
            const b = where.get(e.to)!;
            const path = e.back
              ? `M${a.x + NODE_W},${a.y + NODE_H / 2} C${a.x + NODE_W + BACK_ROOM},${a.y} ${b.x + NODE_W + BACK_ROOM},${b.y + NODE_H} ${b.x + NODE_W},${b.y + NODE_H / 2}`
              : `M${a.x + NODE_W / 2},${a.y + NODE_H} C${a.x + NODE_W / 2},${a.y + NODE_H + GAP_Y / 2} ${b.x + NODE_W / 2},${b.y - GAP_Y / 2} ${b.x + NODE_W / 2},${b.y}`;
            const label = e.round > 1 ? `×${formatCount(e.round)}` : e.count > 1 ? formatCount(e.count) : null;
            const lx = e.back ? Math.max(a.x, b.x) + NODE_W + BACK_ROOM * 0.6 : (a.x + b.x + NODE_W) / 2 + 6;
            const ly = e.back ? (a.y + b.y + NODE_H) / 2 : (a.y + NODE_H + b.y) / 2;
            return (
              <g key={`${e.from}\u0001${e.to}`}>
                <path
                  d={path}
                  fill="none"
                  stroke="currentColor"
                  strokeWidth={1.5}
                  strokeDasharray={e.back ? "4 3" : undefined}
                  markerEnd={`url(#${arrow})`}
                />
                {label && (
                  <text x={lx} y={ly} dominantBaseline="middle" className="fill-foreground text-[11px] tabular-nums">
                    {label}
                  </text>
                )}
              </g>
            );
          })}
        </svg>
        {graph.nodes.map((node) => {
          const { x, y } = where.get(node.key)!;
          const picked = chosen !== undefined && node.spans.includes(chosen);
          const failed = node.failed > 0;
          const heard = heardAs(graph, byKey, node);
          const body = (
            <>
              <KindIcon span={{ kind: node.kind, status: failed ? SpanStatus.Error : SpanStatus.Ok }} />
              <span className="min-w-0 flex-1 truncate">{node.title}</span>
              {node.loop > 1 && (
                <span className="flex shrink-0 items-center gap-0.5 text-xs font-medium tabular-nums">
                  <Repeat aria-hidden="true" className="size-3" />×{formatCount(node.loop)}
                </span>
              )}
              {node.spans.length > Math.max(1, node.loop) && (
                <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
                  {formatCount(node.spans.length)}
                </span>
              )}
            </>
          );
          const look = cn(
            "flex size-full items-center gap-1.5 rounded-md border bg-card px-2 text-left text-sm",
            failed ? "border-destructive" : "border-border",
            picked && "bg-muted ring-2 ring-ring",
          );
          return (
            <div
              key={node.key}
              role="listitem"
              data-node={node.key}
              className="absolute"
              style={{ left: x, top: y, width: NODE_W, height: NODE_H }}
            >
              {onChoose ? (
                <button
                  type="button"
                  title={nodeWords(node)}
                  aria-label={heard}
                  aria-pressed={picked}
                  onClick={() => choose(node)}
                  className={cn(look, "cursor-pointer hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring")}
                >
                  {body}
                </button>
              ) : (
                <div title={nodeWords(node)} className={look}>
                  <span className="sr-only">{heard}</span>
                  <span aria-hidden="true" className="contents">
                    {body}
                  </span>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
