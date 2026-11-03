import { describe, expect, it } from "vitest";

import {
  COST_DASHBOARD,
  DASHBOARD_CHARTS_MAX,
  SERIES_MAX,
  barsOf,
  chartOf,
  chartWord,
  chartsOf,
  dashboardsHref,
  groupTotals,
  sameCharts,
  spendSeries,
} from "./dashboards";

const point = (
  at: string,
  value: string | undefined,
  spend: number,
  runs: number,
  failed: number,
  others = false,
) => ({
  at,
  value,
  others,
  spans: runs,
  failed,
  runs,
  failed_runs: failed,
  input_tokens: 100,
  output_tokens: 10,
  cache_read_input_tokens: 0,
  cache_write_input_tokens: 0,
  spend,
  run_duration_ms: { p50: 1_000, p95: 2_000, p99: 3_000 },
});

describe("the dashboards page's numbers", () => {
  it("draws each hour's runs as those that ended well and those that failed", () => {
    expect(barsOf([point("2026-11-02T00:00:00Z", undefined, 1.5, 4, 1)])).toEqual([
      {
        at: "2026-11-02T00:00:00Z",
        spend: 1.5,
        input: 100,
        output: 10,
        ok: 3,
        failed: 1,
        p50: 1_000,
        p95: 2_000,
        p99: 3_000,
      },
    ]);
  });

  it("totals each group over the window, the costliest first, none and the folded rest after them", () => {
    const totals = groupTotals([
      point("2026-11-01T00:00:00Z", "gpt-5", 1, 2, 0),
      point("2026-11-02T00:00:00Z", "gpt-5", 2, 1, 1),
      point("2026-11-02T00:00:00Z", "claude", 5, 1, 0),
      point("2026-11-02T00:00:00Z", "", 9, 1, 0, true),
      point("2026-11-02T00:00:00Z", "", 0, 1, 0),
    ]);
    expect(totals.map((t) => [t.value, t.spend, t.runs, t.failedRuns])).toEqual([
      [{ kind: "value", value: "claude" }, 5, 1, 0],
      [{ kind: "value", value: "gpt-5" }, 3, 3, 1],
      [{ kind: "others" }, 9, 1, 0],
      [{ kind: "none" }, 0, 1, 0],
    ]);
    expect(totals[1]?.tokens).toBe(220);
  });
});

describe("a dashboard's charts", () => {
  it("reads each chart the URL names, once and in order, and none it does not draw", () => {
    expect(
      chartsOf([
        "tokens",
        "spend:user",
        "spend:tag:team:eu",
        "tokens",
        "latency:model",
        "spend:tool",
        "spend:user:x",
        "spend:tag: ",
        `spend:tag:${"k".repeat(65)}`,
        "cost",
      ]),
    ).toEqual([
      { measure: "tokens" },
      { measure: "spend", group: "user" },
      { measure: "spend", group: "tag", tag: "team:eu" },
    ]);
    expect(chartsOf(undefined)).toBeNull();
    expect(chartsOf(["cost"])).toBeNull();
    expect(chartsOf("runs")).toEqual([{ measure: "runs" }]);
  });

  it("writes each chart back as it reads, and holds a dashboard to the server's twelve", () => {
    for (const word of ["spend", "first_token", "spend:session", "spend:tag:team:eu"]) {
      const chart = chartOf(word);
      expect(chart && chartWord(chart)).toBe(word);
    }
    const many = Array.from({ length: 20 }, (_, i) => `spend:tag:k${i}`);
    expect(chartsOf(many)).toHaveLength(DASHBOARD_CHARTS_MAX);
    expect(sameCharts(COST_DASHBOARD, chartsOf(COST_DASHBOARD.map(chartWord)) ?? [])).toBe(true);
    expect(sameCharts(COST_DASHBOARD, [...COST_DASHBOARD].reverse())).toBe(false);
  });

  it("opens a saved dashboard on its charts over the window shown", () => {
    expect(
      dashboardsHref("acme", "web", 30, [{ measure: "spend" }, { measure: "spend", group: "user" }], "v-1"),
    ).toBe("/acme/web/dashboards?days=30&view=v-1&chart=spend&chart=spend%3Auser");
  });
});

describe("a split's spend over time", () => {
  it("colours the values that spent most, in that order, and sums the rest, none and the folded as Other", () => {
    const values = Array.from({ length: SERIES_MAX + 2 }, (_, i) => `v${i}`);
    const points = [
      ...values.map((value, i) => point("2026-11-01T00:00:00Z", value, i + 1, 1, 0)),
      point("2026-11-02T00:00:00Z", "v8", 2, 1, 0),
      point("2026-11-02T00:00:00Z", "", 0.5, 1, 0),
      point("2026-11-02T00:00:00Z", "", 0.25, 1, 0, true),
    ];
    const { series, bars } = spendSeries(points, (value) => (value.kind === "value" ? value.value : value.kind));
    expect(series.map((each) => [each.label, each.color])).toEqual([
      ["v8", "var(--series-1)"],
      ["v7", "var(--series-2)"],
      ["v6", "var(--series-3)"],
      ["v5", "var(--series-4)"],
      ["v4", "var(--series-5)"],
      ["v3", "var(--series-6)"],
      ["v2", "var(--series-7)"],
      ["Other", "var(--mark-faint)"],
    ]);
    expect(bars).toEqual([
      { at: "2026-11-01T00:00:00Z", s1: 9, s2: 8, s3: 7, s4: 6, s5: 5, s6: 4, s7: 3, other: 1 + 2 },
      { at: "2026-11-02T00:00:00Z", s1: 2, other: 0.75 },
    ]);
  });

  it("draws no Other where every value has its colour", () => {
    const { series } = spendSeries([point("2026-11-01T00:00:00Z", "a", 1, 1, 0)], () => "a");
    expect(series.map((each) => each.key)).toEqual(["s1"]);
  });
});
