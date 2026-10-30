import { describe, expect, it } from "vitest";

import type { TraceLayout, TraceSpan } from "@/lib/server/entities/traces";
import { spanTree, treeRows } from "@/lib/trace-tree";
import { DEFAULT_COLUMNS } from "@/lib/traces";

import {
  DEFAULT_LAYOUT,
  OPEN_ALL_UP_TO,
  OPEN_AT_MOST,
  addName,
  choiceOfDefault,
  choiceOfLayout,
  depthAdmits,
  edited,
  filterAdmits,
  layoutOf,
  layoutQueryOf,
  openingFor,
  parseLayoutUi,
  readLayoutChoice,
  readLayoutOptions,
  resolveLayout,
  sameLayout,
  shutBranches,
  spanTitle,
  traceOpensByDefault,
  tracesOpenedFirst,
  visibleTree,
  withLayoutParams,
  type LayoutOptions,
} from "./layout";

const VIEW = "11111111-1111-4111-8111-111111111111";
const OTHER = "22222222-2222-4222-8222-222222222222";

/** The default, and the default customized all the way to steps and calls,
 *  the failed open on a timeline with the list's columns. */
const SIMPLE = DEFAULT_LAYOUT;
const COMPLEX: LayoutOptions = {
  ...DEFAULT_LAYOUT,
  depth: "calls",
  open: "failed",
  timeline: true,
  columns: DEFAULT_COLUMNS,
  messages: false,
};

function saved(over: Partial<TraceLayout> = {}): TraceLayout {
  return {
    id: VIEW,
    kind: "layout",
    name: "Review",
    query: {
      depth: "calls",
      open: "failed",
      timeline: true,
      pinned: [" plan ", "plan", "search_docs"],
      filter: { kind: ["tool_call"], status: ["error"], name: ["search_docs"] },
      messages: false,
      scores: true,
    },
    columns: ["tokens", "started", "error_type"],
    created_at: "2026-10-17T00:00:00Z",
    updated_at: "2026-10-17T00:00:00Z",
    ...over,
  };
}

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
    cost: null,
    time_to_first_token: null,
    user_id: "",
    session_id: "",
    environment: "",
    release: "",
    tags: {},
    sample_rate: null,
    attributes: {},
    units: 1,
    received_at: "2026-10-11T03:00:01.000Z",
    ...over,
  };
}

// A run, a step with a model call and a failed tool call inside it, a second
// step with one call, and a subagent's run straight under the run.
const SPANS = [
  span({ span_id: "1", kind: "run", name: "nightly-report" }),
  span({ span_id: "2", kind: "step", name: "plan", parent_span_id: "1", started_at: "2026-10-11T03:00:00.010Z" }),
  span({ span_id: "3", kind: "model_call", name: "chat", model: "claude-sonnet-5-5", parent_span_id: "2", started_at: "2026-10-11T03:00:00.020Z" }),
  span({ span_id: "4", kind: "tool_call", name: "search_docs", status: "error", error_type: "TimeoutError", parent_span_id: "2", started_at: "2026-10-11T03:00:00.030Z" }),
  span({ span_id: "5", kind: "step", name: "answer", parent_span_id: "1", started_at: "2026-10-11T03:00:00.500Z" }),
  span({ span_id: "6", kind: "model_call", name: "chat", model: "claude-haiku-5-5", parent_span_id: "5", started_at: "2026-10-11T03:00:00.510Z" }),
  span({ span_id: "7", kind: "run", name: "helper", agent: "helper", parent_span_id: "1", started_at: "2026-10-11T03:00:00.800Z" }),
];

const names = (options: LayoutOptions) =>
  treeRows(visibleTree(spanTree(SPANS), options), new Set()).map((n) => n.span.name);

describe("the default layout", () => {
  it("is each trace's line alone, a turn of a conversation, in the list's order", () => {
    expect(DEFAULT_LAYOUT).toMatchObject({ depth: "trace", open: "none", timeline: false, messages: true, scores: true });
    expect(DEFAULT_LAYOUT.columns).toEqual([
      "started",
      "user",
      "status",
      "error_type",
      "tokens",
      "duration",
      "models",
      "tools",
    ]);
    expect(DEFAULT_LAYOUT.pinned).toEqual([]);
    expect(choiceOfDefault()).toEqual({ view: null, options: DEFAULT_LAYOUT });
  });

  it("compares layouts by what they show, a list's values in any order", () => {
    const a = { ...COMPLEX, pinned: ["plan", "answer"], columns: ["started", "status"] as const };
    const b = { ...COMPLEX, pinned: ["answer", "plan"], columns: ["status", "started"] as const };
    expect(sameLayout(a, b)).toBe(true);
    expect(sameLayout(a, { ...b, pinned: ["plan"] })).toBe(false);
    expect(sameLayout(a, { ...b, filter: { kind: ["step"], status: [], name: [] } })).toBe(false);
  });
});

describe("a saved layout", () => {
  it("opens as it was saved, its lists as the page keeps them", () => {
    expect(layoutOf(saved())).toEqual({
      depth: "calls",
      open: "failed",
      timeline: true,
      columns: ["started", "error_type", "tokens"],
      pinned: ["plan", "search_docs"],
      filter: { kind: ["tool_call"], status: ["error"], name: ["search_docs"] },
      messages: false,
      scores: true,
    });
    const nine = saved({
      query: { ...saved().query, pinned: Array.from({ length: 9 }, (_, i) => `s${i}`) },
    });
    expect(layoutOf(nine).pinned).toHaveLength(8);
  });

  it("is saved as its query and its columns, the query every option but the columns", () => {
    const query = layoutQueryOf(layoutOf(saved()));
    expect(query).toEqual({
      depth: "calls",
      open: "failed",
      timeline: true,
      pinned: ["plan", "search_docs"],
      filter: { kind: ["tool_call"], status: ["error"], name: ["search_docs"] },
      messages: false,
      scores: true,
    });
    expect("columns" in query).toBe(false);
  });
});

describe("what the browser kept", () => {
  it("reads a layout whole, or not at all", () => {
    expect(readLayoutOptions(COMPLEX)).toEqual(COMPLEX);
    expect(readLayoutOptions(JSON.parse(JSON.stringify(SIMPLE)))).toEqual(SIMPLE);
    expect(readLayoutOptions(null)).toBeNull();
    expect(readLayoutOptions({ ...SIMPLE, depth: "spans" })).toBeNull();
    expect(readLayoutOptions({ ...SIMPLE, open: "errors" })).toBeNull();
    expect(readLayoutOptions({ ...SIMPLE, timeline: "yes" })).toBeNull();
    expect(readLayoutOptions({ ...SIMPLE, filter: null })).toBeNull();
    // A column or a kind this build does not know is left out, not applied blind.
    expect(
      readLayoutOptions({
        ...COMPLEX,
        columns: ["spend", "tokens", "started"],
        filter: { kind: ["call", "step"], status: ["maybe"], name: [" a ", "a", 3] },
      }),
    ).toEqual({
      ...COMPLEX,
      columns: ["started", "tokens"],
      filter: { kind: ["step"], status: [], name: ["a"] },
    });
  });

  it("reads a choice with the saved layout it began from, and passes over an older build's keys", () => {
    expect(readLayoutChoice({ view: null, options: COMPLEX })).toEqual({ view: null, options: COMPLEX });
    expect(readLayoutChoice({ view: VIEW, options: SIMPLE })).toEqual({ view: VIEW, options: SIMPLE });
    expect(readLayoutChoice({ base: "complex", view: null, options: COMPLEX })).toEqual({
      view: null,
      options: COMPLEX,
    });
    expect(readLayoutChoice({ view: "not-a-view", options: SIMPLE })).toEqual({ view: null, options: SIMPLE });
    expect(readLayoutChoice({ view: null, options: {} })).toBeNull();
  });
});

describe("where a page's layout comes from", () => {
  it("reads the link's saved layout, and drops what is none", () => {
    expect(parseLayoutUi({ view: VIEW })).toEqual({ view: VIEW });
    expect(parseLayoutUi({ view: [VIEW, OTHER] })).toEqual({ view: VIEW });
    expect(parseLayoutUi({ layout: "complex", view: "mine" })).toEqual({});
    expect(parseLayoutUi({})).toEqual({});
  });

  it("takes the link first, then the browser's last choice, then the project's default, then the default", () => {
    const layouts = [saved()];
    const kept = { view: null, options: { ...COMPLEX, timeline: false } };

    expect(resolveLayout({ view: VIEW }, kept, null, layouts)).toEqual({
      choice: choiceOfLayout(saved()),
      source: "link",
    });
    // A link naming a layout the project no longer keeps is as if left out.
    expect(resolveLayout({ view: OTHER }, kept, null, layouts)).toEqual({ choice: kept, source: "browser" });
    expect(resolveLayout({}, kept, { view_id: VIEW }, layouts)).toEqual({ choice: kept, source: "browser" });
    expect(resolveLayout({}, null, { view_id: VIEW }, layouts)).toEqual({
      choice: choiceOfLayout(saved()),
      source: "project",
    });
    // The project's default layout gone, the default holds; nothing known, the default.
    expect(resolveLayout({}, null, { view_id: OTHER }, layouts)).toEqual({
      choice: choiceOfDefault(),
      source: "project",
    });
    expect(resolveLayout({}, null, { view_id: null }, layouts).choice).toEqual(choiceOfDefault());
    expect(resolveLayout({}, null, null, []).choice).toEqual(choiceOfDefault());
  });

  it("keeps a choice whose saved layout is gone as a cut of its own", () => {
    const options = { ...COMPLEX, pinned: ["plan"] };
    expect(resolveLayout({}, { view: OTHER, options }, null, [saved()]).choice).toEqual({
      view: null,
      options,
    });
  });

  it("says whether the options moved off what they began from", () => {
    const layouts = [saved()];
    expect(edited(choiceOfLayout(saved()), layouts)).toBe(false);
    expect(edited({ ...choiceOfLayout(saved()), options: COMPLEX }, layouts)).toBe(true);
    expect(edited(choiceOfDefault(), layouts)).toBe(false);
    expect(edited({ view: null, options: { ...SIMPLE, timeline: true } }, layouts)).toBe(true);
    expect(edited({ view: null, options: { ...SIMPLE, columns: [...SIMPLE.columns].reverse() } }, layouts)).toBe(false);
    // A saved layout gone is judged against the default.
    expect(edited({ view: OTHER, options: SIMPLE }, layouts)).toBe(false);
    expect(edited({ view: OTHER, options: COMPLEX }, layouts)).toBe(true);
  });

  it("writes the link's part of the query string as the choice is", () => {
    expect(withLayoutParams("?days=30&view=x", choiceOfLayout(saved()), false)).toBe(`?days=30&view=${VIEW}`);
    expect(withLayoutParams("?view=x&days=7", choiceOfLayout(saved()), true)).toBe("?days=7");
    expect(withLayoutParams("?days=30", choiceOfDefault(), false)).toBe("?days=30");
    expect(withLayoutParams("", choiceOfDefault(), true)).toBe("");
  });
});

describe("what a layout shows of a trace", () => {
  it("opens a trace as far as the depth says", () => {
    expect(depthAdmits("trace", { kind: "step" })).toBe(false);
    expect(depthAdmits("steps", { kind: "step" })).toBe(true);
    expect(depthAdmits("steps", { kind: "run" })).toBe(true);
    expect(depthAdmits("steps", { kind: "model_call" })).toBe(false);
    expect(depthAdmits("calls", { kind: "tool_call" })).toBe(true);
    expect(names({ ...SIMPLE })).toEqual(["nightly-report"]);
    expect(names({ ...SIMPLE, depth: "steps" })).toEqual(["nightly-report", "plan", "answer", "helper"]);
    expect(names({ ...SIMPLE, depth: "calls" })).toEqual([
      "nightly-report",
      "plan",
      "chat",
      "search_docs",
      "answer",
      "chat",
      "helper",
    ]);
  });

  it("shows the calls the filter admits, a span hidden taking its spans with it", () => {
    const filter = { kind: ["tool_call" as const], status: [], name: [] };
    expect(filterAdmits(filter, SPANS[3]!)).toBe(true);
    expect(filterAdmits(filter, SPANS[2]!)).toBe(false);
    expect(filterAdmits({ kind: [], status: ["error"], name: [] }, SPANS[3]!)).toBe(true);
    expect(filterAdmits({ kind: [], status: [], name: ["chat"] }, SPANS[3]!)).toBe(false);
    // Only the tool calls: the steps go, and their calls with them.
    expect(names({ ...COMPLEX, filter })).toEqual(["nightly-report"]);
    const steps = { kind: ["step" as const, "tool_call" as const], status: [], name: [] };
    expect(names({ ...COMPLEX, filter: steps })).toEqual(["nightly-report", "plan", "search_docs", "answer"]);
    const tree = visibleTree(spanTree(SPANS), { ...COMPLEX, filter: steps });
    expect(tree[0]!.children.map((c) => [c.position, c.siblings])).toEqual([
      [1, 2],
      [2, 2],
    ]);
  });

  it("starts the branches shut as the layout opens them, and shut whatever it says past 400 spans", () => {
    const nodes = spanTree(SPANS);
    expect([...shutBranches(nodes, "none")].sort()).toEqual(["2", "5"]);
    expect([...shutBranches(nodes, "failed")]).toEqual(["5"]);
    expect(shutBranches(nodes, "all").size).toBe(0);
    expect(openingFor(OPEN_ALL_UP_TO, "all")).toBe("all");
    expect(openingFor(OPEN_ALL_UP_TO + 1, "all")).toBe("none");
    expect(openingFor(OPEN_ALL_UP_TO + 1, "failed")).toBe("none");
  });

  it("opens a session's traces as the layout says, the first twenty at most", () => {
    const ok = { status: "ok" as const, errors: 0 };
    const failed = { status: "error" as const, errors: 1 };
    const recovered = { status: "ok" as const, errors: 1 };
    expect(traceOpensByDefault({ open: "none", depth: "calls" }, failed)).toBe(false);
    expect(traceOpensByDefault({ open: "failed", depth: "calls" }, failed)).toBe(true);
    expect(traceOpensByDefault({ open: "failed", depth: "calls" }, recovered)).toBe(true);
    expect(traceOpensByDefault({ open: "failed", depth: "calls" }, ok)).toBe(false);
    expect(traceOpensByDefault({ open: "all", depth: "calls" }, ok)).toBe(true);
    expect(traceOpensByDefault({ open: "all", depth: "trace" }, ok)).toBe(false);
    const traces = Array.from({ length: 30 }, (_, i) => ({ run_id: `run-${i}`, ...ok }));
    expect(tracesOpenedFirst({ open: "all", depth: "steps" }, traces).size).toBe(OPEN_AT_MOST);
    expect(tracesOpenedFirst({ open: "failed", depth: "steps" }, [{ run_id: "a", ...ok }, { run_id: "b", ...failed }])).toEqual(
      new Set(["b"]),
    );
  });

  it("names a span by what it is", () => {
    expect(spanTitle(SPANS[2]!)).toBe("claude-sonnet-5-5");
    expect(spanTitle(SPANS[3]!)).toBe("search_docs");
    expect(spanTitle(SPANS[6]!)).toBe("helper");
    expect(spanTitle(SPANS[1]!)).toBe("plan");
    expect(spanTitle({ kind: "model_call", name: "chat", model: "" })).toBe("chat");
  });

  it("adds a name once, trimmed, up to the most a list holds", () => {
    expect(addName([], " plan ", 8)).toEqual(["plan"]);
    expect(addName(["plan"], "plan", 8)).toEqual(["plan"]);
    expect(addName(["plan"], "  ", 8)).toEqual(["plan"]);
    const eight = Array.from({ length: 8 }, (_, i) => `s${i}`);
    expect(addName(eight, "s9", 8)).toEqual(eight);
    expect(addName(eight, "s9", 50)).toHaveLength(9);
  });
});
