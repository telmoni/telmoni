import { describe, expect, it } from "vitest";

import { barsOf, groupTotals } from "./dashboards";

const point = (at: string, value: string | undefined, spend: number, runs: number, failed: number) => ({
  at,
  value,
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

  it("totals each group over the window, the costliest first", () => {
    const totals = groupTotals([
      point("2026-11-01T00:00:00Z", "gpt-5", 1, 2, 0),
      point("2026-11-02T00:00:00Z", "gpt-5", 2, 1, 1),
      point("2026-11-02T00:00:00Z", "claude", 5, 1, 0),
      point("2026-11-02T00:00:00Z", undefined, 0, 1, 0),
    ]);
    expect(totals.map((t) => [t.value, t.spend, t.runs, t.failedRuns])).toEqual([
      ["claude", 5, 1, 0],
      ["gpt-5", 3, 3, 1],
      ["", 0, 1, 0],
    ]);
    expect(totals[1]?.tokens).toBe(220);
  });
});
