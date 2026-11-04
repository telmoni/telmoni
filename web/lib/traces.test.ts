import type { ReactElement } from "react";
import { describe, expect, it } from "vitest";

import { cellOf } from "@/components/traces/cells";
import type { Trace, TraceSpan, TraceView } from "@/lib/server/entities/traces";
import { testSpan } from "@/lib/span-fixture";
import { TraceColumn } from "@/lib/types/enums";

import {
  COLUMN_MENU,
  DEFAULT_COLUMNS,
  TRACE_COLUMNS,
  activeFilters,
  askedNote,
  currentSort,
  downloadHref,
  fits,
  formatCost,
  formatDuration,
  isTagPair,
  listDownloadHref,
  listPageHref,
  nextSort,
  pageCount,
  parseFilters,
  parseListQuery,
  parseTraceQuery,
  parseTraceUi,
  presetActive,
  presetToggled,
  SEARCH_MAX,
  queryOfView,
  shortId,
  showsView,
  orderedColumns,
  tagKeys,
  tagValues,
  timelineBars,
  toggled,
  traceHref,
  compareHref,
  tracesHref,
  unfiltered,
  viewHref,
  viewQueryOf,
  without,
} from "./traces";

const span = (over: Partial<TraceSpan> & Pick<TraceSpan, "span_id" | "kind">) =>
  testSpan({ name: over.kind, ...over });

const VIEW: TraceView = {
  id: "0b6f2c1e-6a4f-4d2b-9e3a-5c7d8e9f0a1b",
  kind: "traces",
  name: "Errors",
  query: { status: ["error"], agent: ["support-bot", "zeta"], sort: "duration", days: 30 },
  columns: [TraceColumn.Started, TraceColumn.Duration],
  created_at: "2026-10-11T03:00:00Z",
  updated_at: "2026-10-11T03:00:00Z",
};

describe("the URL as the list's state", () => {
  it("reads a score and its bounds, a chip that clears them together, and bounds alone as none", () => {
    const query = parseTraceQuery({ score: " relevance ", score_min: "0.5", score_max: "x" });
    expect(query).toMatchObject({ score: "relevance", score_min: 0.5 });
    expect(query.score_max).toBeUndefined();
    expect(parseTraceQuery({ score_min: "0.5" })).toEqual({});
    expect(parseTraceQuery({ score: "x".repeat(65) })).toEqual({});
    const chips = activeFilters({ score: "relevance", score_min: 0.5, score_max: 0.9 });
    expect(chips).toEqual([{ filter: "score", label: "Score", value: "relevance", text: "relevance ≥ 0.5 ≤ 0.9" }]);
    expect(without({ score: "relevance", score_min: 0.5, agent: ["a"] }, "score")).toEqual({ agent: ["a"] });
  });

  it("reads each filter's values, the sort, the window and the page, and drops what the server would refuse", () => {
    expect(
      parseTraceQuery({
        agent: [" nightly-report ", "support-bot", "nightly-report"],
        user: "9f86d081884c7d659a2feaa0c55ad015",
        session: ["conv-1", "conv-2"],
        status: ["error", "maybe", "cancelled"],
        model: "gpt-x",
        tool: "search_docs",
        environment: ["production", "staging"],
        release: "2026.10.18",
        tag: ["team:search", "untagged", ":nameless", "a:b:c"],
        prompt: "triage-v2",
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
      environment: ["production", "staging"],
      release: ["2026.10.18"],
      tag: ["team:search", "a:b:c"],
      prompt: ["triage-v2"],
      sort: "model_calls",
      dir: "desc",
      days: 30,
      limit: 100,
      page: 3,
    });
    expect(
      parseTraceQuery({
        status: "maybe",
        sort: "price",
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

  it("drops a value past the length the server takes for its filter, counting characters as it does", () => {
    const at = (n: number) => "s".repeat(n);
    expect(
      parseTraceQuery({
        session: [at(200), at(201)],
        agent: [at(128), at(129)],
        tool: [at(145), at(146)],
        user: "😀".repeat(128),
        prompt: at(1_025),
        tag: [`${at(64)}:${at(256)}`, `${at(65)}:v`],
      }),
    ).toEqual({
      session: [at(200)],
      agent: [at(128)],
      tool: [at(145)],
      user: ["😀".repeat(128)],
      tag: [`${at(64)}:${at(256)}`],
    });
  });

  it("measures a typed value in characters, so an id of 128 emoji fits as the server takes it", () => {
    expect(fits("user", "😀".repeat(128))).toBe(true);
    expect(fits("user", "u".repeat(129))).toBe(false);
    expect(fits("tag", `${"k".repeat(64)}:${"v".repeat(256)}`)).toBe(true);
    expect(fits("tag", `${"k".repeat(64)}:${"v".repeat(257)}`)).toBe(false);
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
    expect(compareHref("acme", "web", "5d4f5c68-0000-4000-8000-000000000001", "5d4f5c68-0000-4000-8000-000000000002")).toBe(
      "/acme/web/traces/compare?a=5d4f5c68-0000-4000-8000-000000000001&b=5d4f5c68-0000-4000-8000-000000000002",
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
    expect(
      activeFilters({
        status: ["error"],
        agent: ["bot", "zeta"],
        tag: ["team:search"],
        environment: ["production"],
        days: 30,
        sort: "user",
      }),
    ).toEqual([
      { filter: "agent", label: "Agent", value: "bot", text: "bot" },
      { filter: "agent", label: "Agent", value: "zeta", text: "zeta" },
      { filter: "status", label: "Status", value: "error", text: "Error" },
      { filter: "environment", label: "Environment", value: "production", text: "production" },
      { filter: "tag", label: "Tag", value: "team:search", text: "team:search" },
    ]);
    expect(activeFilters({})).toEqual([]);
  });

  // ⚠ A search of kept content rides in the URL like a filter: named first,
  // taken off by its own ×, gone with every filter, and dropped past what
  // the server takes.
  it("reads a search, names it first, and takes it off alone or with every filter", () => {
    const query = parseTraceQuery({ q: "  where is my refund ", agent: "bot", page: "3" });
    expect(query.q).toBe("where is my refund");
    expect(activeFilters(query)[0]).toEqual({
      filter: "q",
      label: "Search",
      value: "where is my refund",
      text: "where is my refund",
    });
    expect(without(query, "q")).toEqual({ agent: ["bot"] });
    expect(unfiltered(query)).toEqual({});
    expect(parseTraceQuery({ q: "x".repeat(SEARCH_MAX + 1) }).q).toBeUndefined();
    expect(parseTraceQuery({ q: "   " }).q).toBeUndefined();
  });

  it("takes a tag as key:value, the key 1 to 64 characters and the value up to 256", () => {
    expect(isTagPair("team:search")).toBe(true);
    expect(isTagPair("team:")).toBe(true);
    expect(isTagPair("a:b:c")).toBe(true);
    expect(isTagPair("untagged")).toBe(false);
    expect(isTagPair(":nameless")).toBe(false);
    expect(isTagPair(`${"k".repeat(65)}:v`)).toBe(false);
    expect(isTagPair(`k:${"v".repeat(257)}`)).toBe(false);
  });

  it("keeps a sort by a tag's key with its direction, ascending first, and drops one the server would refuse", () => {
    expect(parseTraceQuery({ sort: "tag:repo", dir: "desc" })).toEqual({ sort: "tag:repo", dir: "desc" });
    expect(parseTraceQuery({ sort: `tag:${"😀".repeat(64)}` })).toEqual({ sort: `tag:${"😀".repeat(64)}` });
    for (const sort of ["tag:", `tag:${"k".repeat(65)}`, "tag:re\u0007po", "tags:repo"]) {
      expect(parseTraceQuery({ sort }), sort).toEqual({});
    }
    expect(tracesHref("acme", "web", parseTraceQuery({ sort: "tag:repo", page: "2" }))).toBe(
      "/acme/web/traces?sort=tag%3Arepo&page=2",
    );
    expect(currentSort({ sort: "tag:repo" })).toEqual({ sort: "tag:repo", dir: "asc" });
    expect(nextSort({ sort: "tag:repo" }, "tag:repo")).toEqual({ sort: "tag:repo", dir: "desc", page: undefined });
    expect(nextSort({ sort: "tokens", page: 3 }, "tag:repo")).toEqual({
      sort: "tag:repo",
      dir: "asc",
      page: undefined,
    });
  });

  it("reads the keys a tag facet names, and the values each takes, each once and in order", () => {
    const pairs = ["team:search", "repo:web", "team:core", "repo:web", "a:b:c", "untagged"];
    expect(tagKeys(pairs)).toEqual(["a", "repo", "team"]);
    expect(tagValues(pairs, "team")).toEqual(["core", "search"]);
    expect(tagValues(pairs, "a")).toEqual(["b:c"]);
    expect(tagValues(pairs, "a:b")).toEqual([]);
    expect(tagValues(pairs, "")).toEqual([]);
  });

  it("offers every column the server names, and shows the list's first nine until a person picks", () => {
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
      "scores",
    ]);
    for (const column of TRACE_COLUMNS) {
      expect(Boolean(column.sort) === Boolean(column.first), column.key).toBe(true);
    }
  });

  it("orders columns as the list does, each once", () => {
    expect(orderedColumns(["tools", "agent", "agent", "run_id"])).toEqual(["agent", "tools"]);
    expect(orderedColumns([])).toEqual([]);
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
    expect(
      downloadHref("acme", "web", { environment: ["production"], tag: ["team:search"], prompt: ["p"] }, "csv"),
    ).toBe("/acme/web/traces/export?environment=production&tag=team%3Asearch&prompt=p&format=csv");
    expect(downloadHref("acme", "web", {}, "json", ["r1", "r2"])).toBe(
      "/acme/web/traces/export?format=json&run=r1&run=r2",
    );
  });
});

describe("the sessions and users lists, which take the trace list's filters and pages", () => {
  it("reads a list's query with its own sorts, dropping the trace list's", () => {
    const sorts = { Session: "session", LastSeen: "last_seen" } as const;
    expect(
      parseListQuery({ sort: "last_seen", status: "error", limit: "25", page: "2" }, sorts),
    ).toEqual({ status: ["error"], sort: "last_seen", limit: 25, page: 2 });
    expect(parseListQuery({ sort: "started" }, sorts)).toEqual({});
    expect(parseFilters({ agent: "zeta", sort: "user", days: "30" })).toEqual({ agent: ["zeta"] });
  });

  it("writes a list's page and its download, the sessions' beside their page rather than under it", () => {
    expect(listPageHref("sessions", "acme", "web", { user: ["u"], page: 2 })).toBe(
      "/acme/web/sessions?user=u&page=2",
    );
    expect(listPageHref("traces", "acme", "web", { status: ["error"] }, { display: "chart" })).toBe(
      "/acme/web/traces?status=error&display=chart",
    );
    expect(
      listDownloadHref("sessions", "acme", "web", { status: ["error"], sort: "tokens", page: 3 }, "csv"),
    ).toBe("/acme/web/session-export?status=error&sort=tokens&format=csv");
    expect(listDownloadHref("users", "acme", "web", { days: 30 }, "json")).toBe(
      "/acme/web/users/export?days=30&format=json",
    );
  });

  it("keeps a filter's every other query key as it ticks and clears values, from the first page", () => {
    const query = { status: ["error" as const], sort: "user", page: 3, limit: 25 };
    expect(toggled(query, "agent", "zeta")).toEqual({
      status: ["error"],
      agent: ["zeta"],
      sort: "user",
      limit: 25,
    });
    expect(unfiltered(query)).toEqual({ sort: "user", limit: 25 });
  });
});

describe("a saved view", () => {
  it("keeps the list's query but its page, and opens on it, named in the URL", () => {
    expect(viewQueryOf({ agent: ["zeta"], sort: "tokens", limit: 25, page: 4 })).toEqual({
      agent: ["zeta"],
      sort: "tokens",
      limit: 25,
    });
    expect(
      viewQueryOf({ environment: ["production"], release: ["1.2"], tag: ["team:a"], prompt: ["p"] }),
    ).toEqual({ environment: ["production"], release: ["1.2"], tag: ["team:a"], prompt: ["p"] });
    expect(queryOfView({ environment: ["staging"], tag: ["untagged", "team:a"] })).toEqual({
      environment: ["staging"],
      tag: ["team:a"],
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

  it("keeps a sort by a tag's key, opens on it, and is shown while the list holds it", () => {
    const listed = parseTraceQuery({ sort: "tag:repo", dir: "desc", page: "2" });
    const view: TraceView = { ...VIEW, query: viewQueryOf(listed) };
    expect(view.query).toEqual({ sort: "tag:repo", dir: "desc" });
    expect(queryOfView(view.query)).toEqual({ sort: "tag:repo", dir: "desc" });
    expect(viewHref("acme", "web", view)).toBe(`/acme/web/traces?sort=tag%3Arepo&dir=desc&view=${VIEW.id}`);
    expect(showsView(listed, view)).toBe(true);
    expect(showsView({ ...listed, dir: "asc" }, view)).toBe(false);
    expect(showsView({ ...listed, sort: "tag:team" }, view)).toBe(false);
    // Ascending is a tag's first way, so a view saved without a direction is shown by it.
    expect(showsView({ sort: "tag:repo", dir: "asc" }, { query: { sort: "tag:repo" } })).toBe(true);
  });
});

describe("a sentence asked of the list", () => {
  it("says what the filters could not, and the values the window holds no trace of", () => {
    expect(askedNote({ query: { status: ["error"] }, unmet: null, dropped: [] })).toBeNull();
    expect(
      askedNote({
        query: { agent: ["billing"] },
        unmet: "slower than five seconds",
        dropped: [
          { filter: "agent", value: "ghost" },
          { filter: "model", value: "gpt-9" },
          { filter: "sort", value: "tier" },
        ],
      }),
    ).toBe(
      'The filters can\'t say "slower than five seconds". No trace in this window has agent ghost, model gpt-9, a tag tier to sort by.',
    );
  });

  it("gives the server's sentence alone where it opened on nothing", () => {
    expect(askedNote({ query: null, unmet: "No traces in this window.", dropped: [] })).toBe(
      "No traces in this window.",
    );
  });

  it("opens the list on the answer as a saved view's query, every value the page drops dropped", () => {
    expect(queryOfView({ agent: ["billing"], status: ["error", "nope" as never], days: 30 })).toEqual({
      agent: ["billing"],
      status: ["error"],
      days: 30,
    });
  });
});

describe("numbers into words", () => {
  it("gives a cost to the cent from a dollar, and a fraction of one to three figures", () => {
    expect(formatCost(1_234.5)).toBe("$1,234.50");
    expect(formatCost(1)).toBe("$1.00");
    expect(formatCost(0)).toBe("$0.00");
    expect(formatCost(0.5)).toBe("$0.50");
    expect(formatCost(0.0123)).toBe("$0.0123");
    expect(formatCost(0.000045)).toBe("$0.000045");
    expect(formatCost(0.123456)).toBe("$0.123");
    expect(formatCost(0.9999)).toBe("$1.00");
  });

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

  it("rounds a duration before it chooses the unit, so none reads as 60 of one", () => {
    expect(formatDuration(999.6)).toBe("1 s");
    expect(formatDuration(59_960)).toBe("1 min");
    expect(formatDuration(119_600)).toBe("2 min");
    expect(formatDuration(3_599_600)).toBe("1 h");
    expect(formatDuration(7_199_999)).toBe("2 h");
  });

  it("shortens an id to eight characters", () => {
    expect(shortId("5d4f5c68-0000-4000-8000-000000000001")).toBe("5d4f5c68");
  });
});

describe("a run's time to first token in the list", () => {
  const column = TRACE_COLUMNS.find((spec) => spec.key === TraceColumn.TimeToFirstToken)!;
  const run = (ms: number | null): Trace => ({
    run_id: "3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d",
    agent: "nightly-report",
    user_id: "",
    session_id: "",
    ended: true,
    status: "ok",
    error_type: "",
    started_at: "2026-10-11T03:00:00Z",
    ended_at: "2026-10-11T03:00:02Z",
    duration_ms: 2000,
    time_to_first_token_ms: ms,
    spans: 2,
    steps: 0,
    model_calls: 1,
    tool_calls: 0,
    errors: 0,
    input_tokens: 10,
    output_tokens: 5,
    cache_read_input_tokens: 0,
    cache_write_input_tokens: 0,
    spend: 0,
    models: [],
    tools: [],
    environments: [],
    releases: [],
    tags: [],
    scores: [],
  });

  it("reads as a duration, and as a dash where none of its calls reported one", () => {
    expect(cellOf(column, run(420), {}, null)).toBe("420 ms");
    const none = cellOf(column, run(null), {}, null) as ReactElement<{ children: string }>;
    expect(none.props.children).toBe("—");
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
