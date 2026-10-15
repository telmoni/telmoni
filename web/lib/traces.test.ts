import { describe, expect, it } from "vitest";

import type { TraceSpan, TraceView } from "@/lib/server/entities/traces";
import { TraceColumn } from "@/lib/types/enums";

import {
  COLUMN_MENU,
  DEFAULT_COLUMNS,
  TRACE_COLUMNS,
  activeFilters,
  currentSort,
  downloadHref,
  formatDuration,
  nextSort,
  pageCount,
  parseTraceQuery,
  parseTraceUi,
  presetActive,
  presetToggled,
  queryOfView,
  shortId,
  showsView,
  timelineBars,
  toggled,
  traceHref,
  tracesHref,
  unfiltered,
  viewHref,
  viewQueryOf,
  without,
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
    session_id: "",
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

const VIEW: TraceView = {
  id: "0b6f2c1e-6a4f-4d2b-9e3a-5c7d8e9f0a1b",
  name: "Errors",
  query: { status: ["error"], agent: ["support-bot", "zeta"], sort: "duration", days: 30 },
  columns: [TraceColumn.Started, TraceColumn.Duration],
  created_at: "2026-10-11T03:00:00Z",
  updated_at: "2026-10-11T03:00:00Z",
};

describe("the URL as the list's state", () => {
  it("reads each filter's values, the sort, the window and the page, and drops what the server would refuse", () => {
    expect(
      parseTraceQuery({
        agent: [" nightly-report ", "support-bot", "nightly-report"],
        user: "9f86d081884c7d659a2feaa0c55ad015",
        session: ["conv-1", "conv-2"],
        status: ["error", "maybe", "cancelled"],
        model: "gpt-x",
        tool: "search_docs",
        sort: "model_calls",
        dir: "desc",
        days: "30",
        limit: "100",
        page: "3",
      }),
    ).toEqual({
      agent: ["nightly-report", "support-bot"],
      user: ["9f86d081884c7d659a2feaa0c55ad015"],
      session: ["conv-1", "conv-2"],
      status: ["error", "cancelled"],
      model: ["gpt-x"],
      tool: ["search_docs"],
      sort: "model_calls",
      dir: "desc",
      days: 30,
      limit: 100,
      page: 3,
    });
    expect(
      parseTraceQuery({
        status: "maybe",
        sort: "spend",
        dir: "sideways",
        days: "0",
        limit: "500",
        page: "1",
        agent: "",
      }),
    ).toEqual({});
    expect(parseTraceQuery({ days: "361" })).toEqual({});
    expect(parseTraceQuery({ page: "1001" })).toEqual({});
    expect(parseTraceQuery({})).toEqual({});
  });

  it("reads the page's own state apart from the list's, which the server never sees", () => {
    expect(parseTraceUi({ display: "chart", view: VIEW.id })).toEqual({ display: "chart", view: VIEW.id });
    expect(parseTraceUi({ display: "table", view: "not-a-view" })).toEqual({});
    expect(parseTraceQuery({ display: "chart", view: VIEW.id })).toEqual({});
  });

  it("writes the list's path with one thing changed, a filter's values given again, and a trace's own", () => {
    expect(tracesHref("acme", "web", {})).toBe("/acme/web/traces");
    expect(tracesHref("acme", "web", { agent: ["nightly-report", "zeta"], days: 30 })).toBe(
      "/acme/web/traces?agent=nightly-report&agent=zeta&days=30",
    );
    expect(
      tracesHref("acme", "web", { agent: ["nightly-report"], user: ["u1"] }, { user: undefined }),
    ).toBe("/acme/web/traces?agent=nightly-report");
    expect(tracesHref("acme", "web", { user: ["a b&c"] })).toBe("/acme/web/traces?user=a+b%26c");
    expect(tracesHref("acme", "web", { page: 2 }, {}, { display: "chart", view: VIEW.id })).toBe(
      `/acme/web/traces?page=2&display=chart&view=${VIEW.id}`,
    );
    expect(traceHref("acme", "web", "5d4f5c68-0000-4000-8000-000000000001")).toBe(
      "/acme/web/traces/5d4f5c68-0000-4000-8000-000000000001",
    );
  });

  it("ticks and unticks a value, and takes off a filter or all of them, from the first page again", () => {
    const query = { agent: ["zeta"], status: ["error" as const], days: 30, sort: "agent" as const, page: 4 };
    expect(toggled(query, "agent", "support-bot")).toEqual({
      agent: ["zeta", "support-bot"],
      status: ["error"],
      days: 30,
      sort: "agent",
    });
    expect(toggled(query, "agent", "zeta")).toEqual({ status: ["error"], days: 30, sort: "agent" });
    expect(without(query, "status")).toEqual({ agent: ["zeta"], days: 30, sort: "agent" });
    expect(without({ model: ["a", "b"] }, "model", "a")).toEqual({ model: ["b"] });
    expect(unfiltered(query)).toEqual({ days: 30, sort: "agent" });
  });

  it("sorts newest first by default, flips a column on a second click, starts each its own way, and turns to the first page", () => {
    expect(currentSort({})).toEqual({ sort: "started", dir: "desc" });
    expect(currentSort({ sort: "agent" })).toEqual({ sort: "agent", dir: "asc" });
    expect(currentSort({ sort: "tokens", dir: "asc" })).toEqual({ sort: "tokens", dir: "asc" });
    expect(currentSort({ sort: "errors" })).toEqual({ sort: "errors", dir: "desc" });
    expect(nextSort({}, "started")).toEqual({ sort: "started", dir: "asc", page: undefined });
    expect(nextSort({}, "user")).toEqual({ sort: "user", dir: "asc", page: undefined });
    expect(nextSort({ sort: "user" }, "user")).toEqual({ sort: "user", dir: "desc", page: undefined });
    expect(nextSort({ sort: "user", dir: "desc" }, "user")).toEqual({ sort: "user", dir: "asc", page: undefined });
    expect(nextSort({ sort: "user" }, "input_tokens")).toEqual({
      sort: "input_tokens",
      dir: "desc",
      page: undefined,
    });
  });

  it("names the filters in force a value at a time, in the sidebar's order", () => {
    expect(activeFilters({ status: ["error"], agent: ["bot", "zeta"], days: 30, sort: "user" })).toEqual([
      { filter: "agent", label: "Agent", value: "bot", text: "bot" },
      { filter: "agent", label: "Agent", value: "zeta", text: "zeta" },
      { filter: "status", label: "Status", value: "error", text: "Error" },
    ]);
    expect(activeFilters({})).toEqual([]);
  });

  it("offers every column the server names, and shows the list's first eight until a person picks", () => {
    expect(TRACE_COLUMNS.map((c) => c.key).sort()).toEqual(Object.values(TraceColumn).sort());
    expect(DEFAULT_COLUMNS).toEqual([
      "started",
      "agent",
      "user",
      "session",
      "status",
      "steps",
      "tokens",
      "duration",
    ]);
    for (const column of TRACE_COLUMNS) {
      expect(Boolean(column.sort) === Boolean(column.first), column.key).toBe(true);
    }
  });

  it("files the columns menu as the table's order, the counts and the tokens each one group", () => {
    const listed = COLUMN_MENU.flatMap((block) =>
      block.kind === "column" ? [block.column.key] : block.columns.map((c) => c.key),
    );
    expect(listed).toEqual(TRACE_COLUMNS.map((c) => c.key));
    const groups = COLUMN_MENU.flatMap((block) =>
      block.kind === "group" ? [[block.label, block.columns.map((c) => c.key)]] : [],
    );
    expect(groups).toEqual([
      ["Counts", ["steps", "model_calls", "tool_calls", "errors", "spans"]],
      ["Tokens", ["input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens", "tokens"]],
    ]);
  });

  it("applies a ready-made query, and takes it off again", () => {
    expect(presetActive({}, "errors")).toBe(false);
    const errors = presetToggled({ agent: ["zeta"], page: 3 }, "errors");
    expect(errors).toEqual({ agent: ["zeta"], status: ["error"] });
    expect(presetActive(errors, "errors")).toBe(true);
    expect(presetToggled(errors, "errors")).toEqual({ agent: ["zeta"] });
    const slow = presetToggled({}, "slow");
    expect(slow).toEqual({ sort: "duration", dir: "desc" });
    expect(presetActive(slow, "slow")).toBe(true);
    expect(presetActive({ sort: "duration" }, "slow")).toBe(true);
    expect(presetActive({ sort: "duration", dir: "asc" }, "slow")).toBe(false);
    expect(presetToggled(slow, "slow")).toEqual({});
    expect(presetToggled(slow, "tokens")).toEqual({ sort: "tokens", dir: "desc" });
  });

  it("counts the pages the total fills, one at least", () => {
    expect(pageCount(0, 50)).toBe(1);
    expect(pageCount(50, 50)).toBe(1);
    expect(pageCount(51, 50)).toBe(2);
    expect(pageCount(10_000_000, 25)).toBe(1_000);
  });

  it("writes a download's path with the list's filters and sort, never its page, or the chosen runs", () => {
    expect(
      downloadHref("acme", "web", { agent: ["zeta"], sort: "tokens", page: 3, limit: 25 }, "csv"),
    ).toBe("/acme/web/traces/export?agent=zeta&sort=tokens&format=csv");
    expect(downloadHref("acme", "web", {}, "json", ["r1", "r2"])).toBe(
      "/acme/web/traces/export?format=json&run=r1&run=r2",
    );
  });
});

describe("a saved view", () => {
  it("keeps the list's query but its page, and opens on it, named in the URL", () => {
    expect(viewQueryOf({ agent: ["zeta"], sort: "tokens", limit: 25, page: 4 })).toEqual({
      agent: ["zeta"],
      sort: "tokens",
      limit: 25,
    });
    expect(queryOfView(VIEW.query)).toEqual({
      agent: ["support-bot", "zeta"],
      status: ["error"],
      sort: "duration",
      days: 30,
    });
    expect(viewHref("acme", "web", VIEW, { display: "chart" })).toBe(
      `/acme/web/traces?agent=support-bot&agent=zeta&status=error&sort=duration&days=30&display=chart&view=${VIEW.id}`,
    );
  });

  it("is shown while the list holds its query, a filter's values in any order, the page aside", () => {
    const shown = queryOfView(VIEW.query);
    expect(showsView(shown, VIEW)).toBe(true);
    expect(showsView({ ...shown, agent: ["zeta", "support-bot"], page: 2 }, VIEW)).toBe(true);
    expect(showsView({ ...shown, dir: "desc" }, VIEW)).toBe(true);
    expect(showsView({ ...shown, agent: ["zeta"] }, VIEW)).toBe(false);
    expect(showsView({ ...shown, days: 7 }, VIEW)).toBe(false);
    expect(showsView({ ...shown, sort: "tokens" }, VIEW)).toBe(false);
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

describe("a trace's spans on one line of time", () => {
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
