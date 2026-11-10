import { describe, expect, it } from "vitest";

import { barsOf } from "./dashboards";

describe("the Spend page's numbers", () => {
  it("draws each hour's traces as those that ended well and those that failed", () => {
    const point = {
      at: "2026-11-02T00:00:00Z",
      spans: 4,
      failed: 1,
      traces: 4,
      failed_traces: 1,
      input_tokens: 100,
      output_tokens: 10,
      cache_read_input_tokens: 0,
      cache_write_input_tokens: 0,
      spend: 1.5,
      trace_duration_ms: { p50: 1_000, p95: 2_000, p99: 3_000 },
    };
    expect(barsOf([point])).toEqual([
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
});
