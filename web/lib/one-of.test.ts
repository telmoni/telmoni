import { describe, expect, it } from "vitest";

import { isOneOf } from "./one-of";

describe("isOneOf", () => {
  it("knows a member of a closed set of words or numbers, and nothing else", () => {
    expect(isOneOf(["rounded", "sharp"] as const, "sharp")).toBe(true);
    expect(isOneOf([0, 30, 60] as const, 30)).toBe(true);
    expect(isOneOf(["rounded", "sharp"] as const, "Sharp")).toBe(false);
    expect(isOneOf([0, 30, 60] as const, "30")).toBe(false);
    expect(isOneOf(["rounded", "sharp"] as const, undefined)).toBe(false);
    expect(isOneOf([] as const, "")).toBe(false);
  });
});
