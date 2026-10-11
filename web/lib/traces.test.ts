import { describe, expect, it } from "vitest";

import type { TraceSpan } from "@/lib/server/entities/traces";

import {
  activeFilters,
  currentSort,
  formatDuration,
  groupSteps,
  nextSort,
  parseTraceQuery,
  shortId,
  timelineBars,
  traceHref,
  tracesHref,
} from "./traces";

function span(over: Partial<TraceSpan> & Pick<TraceSpan, "span_id" | "kind">): TraceSpan {
  return {
    parent_span_id: "",
    trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
    agent: "nightly-report",
    name: over.kind,
    started_at: "2026-10-11T03:00:00.000Z",
    ended_at: "2026-10-11T03:00:01.000Z",
    status: "ok",
    error_type: "",
    model: "",
    provider: "",
    input_tokens: 0,
    output_tokens: 0,
    cache_read_input_tokens: 0,
    cache_write_input_tokens: 0,
    user_id: "",
    conversation_id: "",
    tags: {},
    bound_stall_after_s: null,
    bound_max_steps: null,
    bound_max_cost_usd: null,
    sample_rate: null,
    attributes: {},
    units: 1,
    received_at: "2026-10-11T03:00:01.000Z",
    ...over,
  };
}

describe("the URL as the list's state", () => {
  it("reads every filter and the sort, and drops what the server would refuse", () => {
    expect(
      parseTraceQuery({
        agent: " nightly-report ",
        user: "9f86d081884c7d659a2feaa0c55ad015",
        conversation: ["conv-1", "conv-2"],
        status: "error",
        model: "gpt-x",
        tool: "search_docs",
        sort: "user",
        dir: "desc",
        days: "30",
        limit: "500",
      }),
    ).toEqual({
      agent: "nightly-report",
      user: "9f86d081884c7d659a2feaa0c55ad015",
      conversation: "conv-1",
      status: "error",
      model: "gpt-x",
      tool: "search_docs",
      sort: "user",
      dir: "desc",
      days: 30,
      limit: 500,
    });
    expect(
      parseTraceQuery({
        status: "maybe",
        sort: "spend",
        dir: "sideways",
        days: "0",
        limit: "9999",
        agent: "",
      }),
    ).toEqual({});
    expect(parseTraceQuery({ days: "361" })).toEqual({});
    expect(parseTraceQuery({ days: "7.5" })).toEqual({});
    expect(parseTraceQuery({})).toEqual({});
  });

  it("writes the list's path with one thing changed, and a trace's own", () => {
    expect(tracesHref("acme", "web", {})).toBe("/acme/web/traces");
    expect(tracesHref("acme", "web", { agent: "nightly-report", days: 30 })).toBe(
      "/acme/web/traces?agent=nightly-report&days=30",
    );
    expect(
      tracesHref("acme", "web", { agent: "nightly-report", user: "u1" }, { user: undefined }),
    ).toBe("/acme/web/traces?agent=nightly-report");
    expect(tracesHref("acme", "web", { user: "a b&c" })).toBe("/acme/web/traces?user=a+b%26c");
    expect(traceHref("acme", "web", "5d4f5c68-0000-4000-8000-000000000001")).toBe(
      "/acme/web/traces/5d4f5c68-0000-4000-8000-000000000001",
    );
  });

  it("sorts newest first by default, flips a column on a second click, and starts each its own way", () => {
    expect(currentSort({})).toEqual({ sort: "started", dir: "desc" });
    expect(currentSort({ sort: "agent" })).toEqual({ sort: "agent", dir: "asc" });
    expect(currentSort({ sort: "tokens", dir: "asc" })).toEqual({ sort: "tokens", dir: "asc" });
    expect(nextSort({}, "started")).toEqual({ sort: "started", dir: "asc" });
    expect(nextSort({}, "user")).toEqual({ sort: "user", dir: "asc" });
    expect(nextSort({ sort: "user" }, "user")).toEqual({ sort: "user", dir: "desc" });
    expect(nextSort({ sort: "user", dir: "desc" }, "user")).toEqual({ sort: "user", dir: "asc" });
    expect(nextSort({ sort: "user" }, "duration")).toEqual({ sort: "duration", dir: "desc" });
  });

  it("names the filters in force, in the table's order", () => {
    expect(activeFilters({ status: "error", agent: "bot", days: 30, sort: "user" })).toEqual([
      { key: "agent", label: "Agent", value: "bot" },
      { key: "status", label: "Status", value: "error" },
    ]);
    expect(activeFilters({})).toEqual([]);
  });
});

describe("numbers into words", () => {
  it("gives a duration in the unit a person reads it in", () => {
    expect(formatDuration(0)).toBe("0 ms");
    expect(formatDuration(340)).toBe("340 ms");
    expect(formatDuration(1_234)).toBe("1.2 s");
    expect(formatDuration(59_949)).toBe("59.9 s");
    expect(formatDuration(123_000)).toBe("2 min 3 s");
    expect(formatDuration(120_000)).toBe("2 min");
    expect(formatDuration(3_840_000)).toBe("1 h 4 min");
    expect(formatDuration(7_200_000)).toBe("2 h");
    expect(formatDuration(-5)).toBe("0 ms");
  });

  it("shortens an id to eight characters", () => {
    expect(shortId("5d4f5c68-0000-4000-8000-000000000001")).toBe("5d4f5c68");
  });
});

describe("a trace as steps and their calls", () => {
  const run = span({ span_id: "01", kind: "run", name: "nightly-report" });
  const step = span({
    span_id: "02",
    kind: "step",
    parent_span_id: "01",
    name: "plan the reply",
    started_at: "2026-10-11T03:00:00.010Z",
  });
  const call = span({
    span_id: "03",
    kind: "model_call",
    parent_span_id: "02",
    name: "chat gpt-x",
    started_at: "2026-10-11T03:00:00.020Z",
  });
  // A call under an exporter's own span under the step still belongs to the step.
  const http = span({
    span_id: "04",
    kind: "span",
    parent_span_id: "02",
    name: "POST /v1/messages",
    started_at: "2026-10-11T03:00:00.030Z",
  });
  const nested = span({
    span_id: "05",
    kind: "tool_call",
    parent_span_id: "04",
    name: "search_docs",
    started_at: "2026-10-11T03:00:00.040Z",
  });
  const direct = span({
    span_id: "06",
    kind: "tool_call",
    parent_span_id: "01",
    name: "fetch_page",
    started_at: "2026-10-11T03:00:00.005Z",
  });

  it("nests each call under the nearest step above it, the run's own first when first", () => {
    const { run: found, groups } = groupSteps([run, step, call, http, nested, direct]);
    expect(found?.span_id).toBe("01");
    expect(groups.map((g) => [g.step?.name ?? null, g.calls.map((c) => c.name)])).toEqual([
      [null, ["fetch_page"]],
      ["plan the reply", ["chat gpt-x", "search_docs"]],
    ]);
  });

  it("lists a step with no calls, and leaves out an empty run group", () => {
    const { groups } = groupSteps([run, step]);
    expect(groups).toEqual([{ step, calls: [] }]);
    expect(groupSteps([]).run).toBeNull();
  });

  it("places every span on the timeline as a share of the whole, by depth", () => {
    const bars = timelineBars([run, step, call]);
    expect(bars.map((b) => b.depth)).toEqual([0, 1, 2]);
    expect(bars[0]).toMatchObject({ offset: 0, width: 100 });
    expect(bars[1]!.offset).toBeCloseTo(1, 5);
    expect(bars[1]!.width).toBeCloseTo(99, 5);
    // A span of no duration is still a sliver.
    const instant = span({
      span_id: "07",
      kind: "step",
      started_at: "2026-10-11T03:00:00.500Z",
      ended_at: "2026-10-11T03:00:00.500Z",
    });
    expect(timelineBars([run, instant])[1]!.width).toBe(0.5);
    expect(timelineBars([])).toEqual([]);
  });
});
