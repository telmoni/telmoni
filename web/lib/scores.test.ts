import { describe, expect, it } from "vitest";

import { commentsOf, formatScoreValue, isSpanId, scoreTitle, scoreWords, scoresLine } from "./scores";

const summary = (over: Partial<Parameters<typeof scoreWords>[0]>) => ({
  name: "helpful",
  data_type: "number" as const,
  mean: 0.75,
  count: 1,
  categories: [],
  ...over,
});

describe("scoreWords", () => {
  it("reads a number's mean, a yes or no's share, and a category's values", () => {
    expect(scoreWords(summary({}))).toBe("0.75");
    expect(scoreWords(summary({ mean: 4 }))).toBe("4");
    expect(scoreWords(summary({ mean: 12.345 }))).toBe("12.3");
    expect(scoreWords(summary({ data_type: "boolean", mean: 1 }))).toBe("yes");
    expect(scoreWords(summary({ data_type: "boolean", mean: 0 }))).toBe("no");
    expect(scoreWords(summary({ data_type: "boolean", mean: 0.5, count: 4 }))).toBe("50% yes");
    expect(scoreWords(summary({ data_type: "category", mean: null, categories: ["polite", "terse"] }))).toBe(
      "polite, terse",
    );
  });

  it("says a number's many scores are their mean", () => {
    expect(scoreTitle(summary({ count: 3 }))).toBe("helpful: the mean of 3 scores");
    expect(scoreTitle(summary({}))).toBe("helpful: 1 score");
    expect(formatScoreValue(-0.5)).toBe("-0.50");
  });

  it("says a row's scores in one line, and nothing of a row with none", () => {
    expect(scoresLine([summary({}), summary({ name: "resolved", data_type: "boolean", mean: 1 })])).toBe(
      "helpful: 0.75, resolved: yes",
    );
    expect(scoresLine([])).toBeUndefined();
  });
});

describe("commentsOf", () => {
  it("reads each score's comment from the content as opened, and nothing else", () => {
    const content: { span_id: string; fields: Record<string, string> }[] = [
      { span_id: "score/7a1c0e52-0000-4000-8000-000000000002", fields: { "telmoni.score.comment": "Right refund." } },
      { span_id: "00f067aa0ba902b7", fields: { "telmoni.score.comment": "a span's own field" } },
      { span_id: "score/empty", fields: {} },
    ];
    expect(commentsOf(content)).toEqual({ "7a1c0e52-0000-4000-8000-000000000002": "Right refund." });
    expect(commentsOf(null)).toEqual({});
  });
});

describe("isSpanId", () => {
  it("knows a span's own id from a comment's and an example's", () => {
    expect(isSpanId("00f067aa0ba902b7")).toBe(true);
    expect(isSpanId("score/7a1c0e52-0000-4000-8000-000000000002")).toBe(false);
    expect(isSpanId("example/7a1c/run/5d4f")).toBe(false);
  });
});
