import { describe, expect, it } from "vitest";

import { DEFAULT_SPEND_LAYOUT, barsOf, readSpendLayout, tokenBarsOf } from "./dashboards";

describe("personal Spend layouts", () => {
  it("keeps a saved order, width and hidden charts", () => {
    const layout = DEFAULT_SPEND_LAYOUT.toReversed().map((row, index) => ({
      ...row, width: index === 0 ? "full" : "half", visible: index !== 1,
    }));
    expect(readSpendLayout(JSON.stringify(layout))).toEqual(layout);
  });

  it.each([
    null,
    "not json",
    JSON.stringify(DEFAULT_SPEND_LAYOUT.slice(1)),
    JSON.stringify(DEFAULT_SPEND_LAYOUT.map(() => DEFAULT_SPEND_LAYOUT[0])),
    JSON.stringify(DEFAULT_SPEND_LAYOUT.map((row) => ({ ...row, width: "invalid" }))),
    JSON.stringify(DEFAULT_SPEND_LAYOUT.map((row) => ({ ...row, measure: "unknown" }))),
  ])("uses the complete default layout when stored data is invalid: %s", (stored) => {
    expect(readSpendLayout(stored)).toEqual(DEFAULT_SPEND_LAYOUT);
  });
});

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
      reasoning_output_tokens: 0,
      spend: 1.5,
      trace_duration_ms: { p50: 1_000, p95: 2_000, p99: 3_000 },
    };
    expect(barsOf([point])).toEqual([
      {
        at: "2026-11-02T00:00:00Z",
        spend: 1.5,
        ok: 3,
        failed: 1,
        p50: 1_000,
        p95: 2_000,
        p99: 3_000,
      },
    ]);
  });

  const tokens = {
    input_tokens: 100,
    output_tokens: 10,
    cache_read_input_tokens: 40,
    cache_write_input_tokens: 5,
    reasoning_output_tokens: 2,
  };
  const point = {
    at: "2026-11-02T00:00:00Z",
    spans: 4,
    failed: 0,
    traces: 4,
    failed_traces: 0,
    ...tokens,
    spend: 1.5,
    trace_duration_ms: { p50: null, p95: null, p99: null },
  };

  it("draws each bar's tokens by model, a model's five counts and no more", () => {
    const [bar] = tokenBarsOf([
      { ...point, value: "__proto__" },
      { ...point, value: "m" },
    ]);
    expect(Object.keys(bar!.tokens_by_model)).toEqual(["__proto__", "m"]);
    expect(bar!.tokens_by_model["m"]).toEqual(tokens);
  });

  it("leaves out the value no token was spent under, and keeps its bar", () => {
    const spent = { ...point, input_tokens: 0, output_tokens: 0, cache_read_input_tokens: 0 };
    expect(
      tokenBarsOf([
        { ...spent, value: "" },
        { ...point, at: "2026-11-03T00:00:00Z", value: "m" },
      ]),
    ).toEqual([
      { at: "2026-11-02T00:00:00Z", tokens_by_model: {} },
      { at: "2026-11-03T00:00:00Z", tokens_by_model: { m: tokens } },
    ]);
  });

  it("draws totals read without a group under one name", () => {
    expect(tokenBarsOf([point])).toEqual([{ at: point.at, tokens_by_model: { "": tokens } }]);
  });
});
