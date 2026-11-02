import { describe, expect, it } from "vitest";

import { formatScoreValue, scoreTitle, scoreWords } from "./scores";

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
});
