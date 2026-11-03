import { describe, expect, it } from "vitest";

import { totalsOf } from "./overview";

const point = (runs: number, failed: number, spend: number) => ({
  at: "2026-11-02T00:00:00Z",
  spans: runs,
  failed,
  runs,
  failed_runs: failed,
  input_tokens: 10,
  output_tokens: 5,
  cache_read_input_tokens: 0,
  cache_write_input_tokens: 0,
  spend,
  duration_ms: { avg: 1, p50: 1, p90: 2, p99: 3 },
  run_duration_ms: { p50: 1, p95: 2, p99: 3 },
});

describe("totalsOf", () => {
  it("sums the window's runs, failures, spend and tokens", () => {
    expect(totalsOf([point(3, 1, 0.5), point(2, 0, 1.25)])).toEqual({
      runs: 5,
      failedRuns: 1,
      spend: 1.75,
      tokens: 30,
    });
    expect(totalsOf([])).toEqual({ runs: 0, failedRuns: 0, spend: 0, tokens: 0 });
  });
});
