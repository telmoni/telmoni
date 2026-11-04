// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { TraceGraph } from "@/components/traces/trace-graph";
import { DEFAULT_LAYOUT, visibleTree, type LayoutOptions } from "@/lib/layout";
import type { TraceSpan } from "@/lib/server/entities/traces";
import { nextSpan } from "@/lib/span-fixture";
import { spanTree } from "@/lib/trace-tree";
import { LayoutDepth, SpanKind, SpanStatus } from "@/lib/types/enums";

import { GRAPH_NODES_MAX, agentGraph, repeats, spanKey } from "./trace-graph";

const timed = nextSpan();
const span = (over: Partial<TraceSpan> & Pick<TraceSpan, "span_id">) => timed({ agent: "support", ...over });

const run = () => span({ span_id: "run", kind: SpanKind.Run, name: "invoke_agent" });
const tool = (id: string, name = "search", over: Partial<TraceSpan> = {}) =>
  span({ span_id: id, parent_span_id: "run", kind: SpanKind.ToolCall, name, ...over });
const model = (id: string) =>
  span({ span_id: id, parent_span_id: "run", kind: SpanKind.ModelCall, name: "chat", model: "gpt-5" });

const EVERY_SPAN: LayoutOptions = { ...DEFAULT_LAYOUT, depth: LayoutDepth.Calls };
const graphOf = (spans: TraceSpan[], options = EVERY_SPAN) =>
  agentGraph(visibleTree(spanTree(spans), options));
const titles = (graph: ReturnType<typeof agentGraph>) => graph.nodes.map((n) => n.title);

describe("repeats", () => {
  it("cuts a sibling list into the stretches that repeat, the longest cover first", () => {
    expect(repeats(["a", "a", "a", "a", "a"])).toEqual([{ start: 0, period: 1, times: 5 }]);
    expect(repeats(["p", "a", "p", "a", "p", "a", "done"])).toEqual([
      { start: 0, period: 2, times: 3 },
      { start: 6, period: 1, times: 1 },
    ]);
    // Four of one is one node four times, never a pair twice.
    expect(repeats(["a", "a", "a", "a"])).toEqual([{ start: 0, period: 1, times: 4 }]);
    expect(repeats(["a", "b", "c"]).map((b) => b.times)).toEqual([1, 1, 1]);
    expect(repeats([])).toEqual([]);
  });
});

describe("agentGraph", () => {
  /** The roadmap's own case: an agent that called one tool five times in a row. */
  it("draws a tool called five times in a row as one node marked ×5", () => {
    const graph = graphOf([run(), model("m1"), ...[1, 2, 3, 4, 5].map((i) => tool(`t${i}`)), model("m2")]);
    expect(titles(graph)).toEqual(["support", "gpt-5", "search"]);
    const search = graph.nodes.find((n) => n.title === "search")!;
    expect(search.loop).toBe(5);
    expect(search.spans).toEqual(["t1", "t2", "t3", "t4", "t5"]);
    expect(graph.edges.some((e) => e.from === e.to)).toBe(false);
    // The model answers after the loop: a way back up to the node seen first.
    const back = graph.edges.find((e) => e.back)!;
    expect([back.from, back.to]).toEqual([search.key, spanKey(model("m2"))]);
  });

  it("draws a stretch of several that repeats once, its way back marked with its rounds", () => {
    const spans = [run()];
    for (let i = 0; i < 3; i++) spans.push(model(`m${i}`), tool(`t${i}`));
    const graph = graphOf(spans);
    const [, chat, search] = graph.nodes;
    expect(graph.edges).toEqual(
      expect.arrayContaining([
        { from: chat!.key, to: search!.key, count: 3, back: false, round: 0 },
        { from: search!.key, to: chat!.key, count: 2, back: true, round: 3 },
      ]),
    );
    expect(graph.nodes.map((n) => n.rank)).toEqual([0, 1, 2]);
  });

  it("tells a step and a tool of one name apart, and counts a node's failures", () => {
    const graph = graphOf([
      run(),
      span({ span_id: "s", parent_span_id: "run", kind: SpanKind.Step, name: "search" }),
      tool("t", "search", { parent_span_id: "s", status: SpanStatus.Error }),
    ]);
    expect(graph.nodes.map((n) => [n.kind, n.title, n.failed])).toEqual([
      [SpanKind.Run, "support", 0],
      [SpanKind.Step, "search", 0],
      [SpanKind.ToolCall, "search", 1],
    ]);
  });

  it("draws only what the layout shows", () => {
    const spans = [run(), model("m1"), tool("t1")];
    expect(titles(graphOf(spans, DEFAULT_LAYOUT))).toEqual(["support"]);
    expect(
      titles(graphOf(spans, { ...EVERY_SPAN, filter: { kind: [SpanKind.ToolCall], status: [], name: [] } })),
    ).toEqual(["support", "search"]);
  });

  it("draws at most its cap of nodes and counts the rest", () => {
    const spans = [run(), ...Array.from({ length: GRAPH_NODES_MAX + 5 }, (_, i) => tool(`t${i}`, `tool-${i}`))];
    const graph = graphOf(spans);
    expect(graph.nodes).toHaveLength(GRAPH_NODES_MAX);
    expect(graph.hidden).toBe(6);
    const drawn = new Set(graph.nodes.map((n) => n.key));
    expect(graph.edges.every((e) => drawn.has(e.from) && drawn.has(e.to))).toBe(true);
  });
});

describe("TraceGraph", () => {
  afterEach(cleanup);

  const looped = () => [run(), model("m1"), ...[1, 2, 3, 4, 5].map((i) => tool(`t${i}`)), model("m2")];
  const draw = (spans: TraceSpan[], onChoose?: (id: string) => void, chosen?: string) =>
    render(
      <TraceGraph
        trace={{ spans, truncated: false }}
        tree={visibleTree(spanTree(spans), EVERY_SPAN)}
        chosen={chosen}
        onChoose={onChoose}
      />,
    );

  /** The roadmap's proof: five calls of one tool in a row, drawn as one cycle marked ×5. */
  it("draws the five calls as one node marked ×5", () => {
    draw(looped());
    const items = screen.getAllByRole("listitem");
    expect(items).toHaveLength(3);
    const search = items.filter((item) => item.textContent?.startsWith("search"));
    expect(search).toHaveLength(1);
    expect(search[0]!.textContent).toContain("×5");
    expect(search[0]!.textContent).toContain("search (tool call), ran 5 times, 5 times in a row");
  });

  it("says how many times a node ran in all where that is more than its loop", () => {
    draw([run(), tool("t1"), tool("t2"), tool("t3"), model("m1"), tool("t4"), tool("t5")]);
    const search = screen.getAllByRole("listitem").find((item) => item.textContent?.startsWith("search"))!;
    expect(search.textContent).toContain("ran 5 times, 3 times in a row");
    expect(search.textContent).toMatch(/×3\s*5$/);
  });

  it("chooses a node's first span, then its next as it is chosen again", () => {
    const onChoose = vi.fn();
    const { rerender } = draw(looped(), onChoose);
    const search = () => screen.getByRole("button", { name: /^search/ });
    fireEvent.click(search());
    expect(onChoose).toHaveBeenLastCalledWith("t1");
    const spans = looped();
    rerender(
      <TraceGraph
        trace={{ spans, truncated: false }}
        tree={visibleTree(spanTree(spans), EVERY_SPAN)}
        chosen="t5"
        onChoose={onChoose}
      />,
    );
    expect(search()).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(search());
    expect(onChoose).toHaveBeenLastCalledWith("t1");
  });

  it("says so where the layout leaves nothing under the run", () => {
    const spans = [run(), tool("t1")];
    render(<TraceGraph trace={{ spans, truncated: false }} tree={visibleTree(spanTree(spans), DEFAULT_LAYOUT)} />);
    expect(screen.getByText("No span of this trace is shown under this layout.")).toBeInTheDocument();
  });
});
