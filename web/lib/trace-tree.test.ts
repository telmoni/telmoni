import { describe, expect, it } from "vitest";

import type { TraceSpan } from "@/lib/server/entities/traces";
import { SpanKind, SpanStatus } from "@/lib/types/enums";

import {
  branchesFrom,
  isUnder,
  matchingRows,
  parentRow,
  spanTree,
  summarizeTrace,
  treeRows,
} from "./trace-tree";

function span(over: Partial<TraceSpan> & Pick<TraceSpan, "span_id">): TraceSpan {
  return {
    parent_span_id: "",
    trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
    agent: "nightly-report",
    kind: SpanKind.Span,
    name: over.span_id,
    started_at: "2026-10-11T03:00:00.000Z",
    ended_at: "2026-10-11T03:00:01.000Z",
    status: SpanStatus.Ok,
    error_type: "",
    model: "",
    provider: "",
    input_tokens: 0,
    output_tokens: 0,
    cache_read_input_tokens: 0,
    cache_write_input_tokens: 0,
    user_id: "",
    session_id: "",
    environment: "",
    release: "",
    tags: {},
    bound_stall_after_s: null,
    bound_max_steps: null,
    bound_max_cost_usd: null,
    sample_rate: null,
    attributes: {},
    units: 1,
    received_at: "2026-10-11T03:00:02.000Z",
    ...over,
  };
}

const at = (second: number) => `2026-10-11T03:00:${String(second).padStart(2, "0")}.000Z`;

// A run with two steps, the second started first in the export's order, and
// a model call and a tool call inside the first step.
const SPANS = [
  span({ span_id: "run", kind: SpanKind.Run, name: "nightly-report", started_at: at(0), ended_at: at(9) }),
  span({ span_id: "step-2", parent_span_id: "run", kind: SpanKind.Step, name: "write", started_at: at(5) }),
  span({ span_id: "step-1", parent_span_id: "run", kind: SpanKind.Step, name: "plan", started_at: at(1) }),
  span({
    span_id: "tool",
    parent_span_id: "step-1",
    kind: SpanKind.ToolCall,
    name: "search",
    started_at: at(3),
  }),
  span({
    span_id: "model",
    parent_span_id: "step-1",
    kind: SpanKind.ModelCall,
    name: "chat",
    model: "gemini-3.1-flash-lite",
    input_tokens: 1200,
    output_tokens: 300,
    started_at: at(2),
  }),
];

const ids = (rows: { span: TraceSpan }[]) => rows.map((r) => r.span.span_id);

describe("a trace's spans as a tree", () => {
  it("nests each span under its parent, siblings in the order they started", () => {
    const tree = spanTree(SPANS);
    expect(tree).toHaveLength(1);
    expect(ids(treeRows(tree, new Set()))).toEqual(["run", "step-1", "model", "tool", "step-2"]);
    expect(treeRows(tree, new Set()).map((r) => r.depth)).toEqual([0, 1, 2, 2, 1]);
  });

  it("stands a span whose parent never arrived at the top, beside the run", () => {
    const orphan = span({ span_id: "late", parent_span_id: "sampled-out", started_at: at(8) });
    expect(ids(spanTree([...SPANS, orphan]))).toEqual(["run", "late"]);
  });

  it("draws spans caught in a cycle once each, rather than nowhere", () => {
    const a = span({ span_id: "a", parent_span_id: "b", started_at: at(1) });
    const b = span({ span_id: "b", parent_span_id: "a", started_at: at(2) });
    const self = span({ span_id: "self", parent_span_id: "self", started_at: at(3) });
    expect(ids(treeRows(spanTree([a, b, self]), new Set()))).toEqual(["a", "b", "self"]);
  });

  it("leaves out what stands under a shut span, and nothing else", () => {
    expect(ids(treeRows(spanTree(SPANS), new Set(["step-1"])))).toEqual(["run", "step-1", "step-2"]);
    expect(ids(treeRows(spanTree(SPANS), new Set(["run"])))).toEqual(["run"]);
  });

  it("finds spans by name, kind or model, each with the spans above it", () => {
    const tree = spanTree(SPANS);
    expect(ids(matchingRows(tree, "search"))).toEqual(["run", "step-1", "tool"]);
    expect(ids(matchingRows(tree, "  GEMINI "))).toEqual(["run", "step-1", "model"]);
    expect(ids(matchingRows(tree, "model call"))).toEqual(["run", "step-1", "model"]);
    expect(ids(matchingRows(tree, "write"))).toEqual(["run", "step-2"]);
    expect(matchingRows(tree, "nothing like it")).toEqual([]);
  });

  it("says where each span stands among those under the same parent", () => {
    const rows = treeRows(spanTree(SPANS), new Set());
    expect(rows.map((r) => `${r.span.span_id}:${r.position}/${r.siblings}`)).toEqual([
      "run:1/1",
      "step-1:1/2",
      "model:1/2",
      "tool:2/2",
      "step-2:2/2",
    ]);
  });

  it("names the branches from a depth down, what a large trace opens with shut", () => {
    const tree = spanTree(SPANS);
    expect([...branchesFrom(tree, 1)]).toEqual(["step-1"]);
    expect([...branchesFrom(tree, 0)].sort()).toEqual(["run", "step-1"]);
  });

  it("knows a span under another by the parents the spans name, a cycle ending the walk", () => {
    const parentOf = new Map(SPANS.map((s) => [s.span_id, s.parent_span_id]));
    expect(isUnder(parentOf, "tool", "step-1")).toBe(true);
    expect(isUnder(parentOf, "tool", "run")).toBe(true);
    expect(isUnder(parentOf, "step-2", "step-1")).toBe(false);
    expect(isUnder(parentOf, "run", "run")).toBe(false);
    expect(isUnder(new Map([["a", "b"], ["b", "a"]]), "a", "c")).toBe(false);
  });

  it("finds the row above a row, the way left steps out of a branch", () => {
    const rows = treeRows(spanTree(SPANS), new Set());
    const row = (id: string) => rows.find((r) => r.span.span_id === id)!;
    expect(parentRow(rows, row("tool"))?.span.span_id).toBe("step-1");
    expect(parentRow(rows, row("step-2"))?.span.span_id).toBe("run");
    expect(parentRow(rows, row("run"))).toBeNull();
  });
});

describe("a trace summed", () => {
  it("names it by the run, spans it end to end, and counts the model calls' tokens alone", () => {
    const summary = summarizeTrace([
      ...SPANS,
      // A step that repeats its call's usage is not counted twice.
      span({ span_id: "echo", kind: SpanKind.Step, input_tokens: 1200, started_at: at(4) }),
    ]);
    expect(summary.root.span_id).toBe("run");
    expect(summary.durationMs).toBe(9_000);
    expect(summary).toMatchObject({
      steps: 3,
      modelCalls: 1,
      toolCalls: 1,
      inputTokens: 1200,
      outputTokens: 300,
    });
  });

  it("spans the run by its times, not by how many digits the server wrote", () => {
    const summary = summarizeTrace([
      span({ span_id: "a", started_at: "2026-10-11T03:00:00.250Z", ended_at: "2026-10-11T03:00:01Z" }),
      span({ span_id: "b", started_at: "2026-10-11T03:00:00Z", ended_at: "2026-10-11T03:00:01.5Z" }),
    ]);
    expect(summary.startedAt).toBe("2026-10-11T03:00:00Z");
    expect(summary.endedAt).toBe("2026-10-11T03:00:01.5Z");
    expect(summary.durationMs).toBe(1_500);
  });

  it("names a run still going by its first span", () => {
    const summary = summarizeTrace(SPANS.slice(1));
    expect(summary.run).toBeNull();
    expect(summary.root.span_id).toBe("step-2");
  });
});
