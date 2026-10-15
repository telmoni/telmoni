import { describe, expect, it, vi } from "vitest";

import { offerPageSteps, pageSteps } from "./use-page-steps";

describe("the steps a page offers j and k", () => {
  it("holds the last offer, and takes back only its own", () => {
    const first = { noun: "trace", next: vi.fn(), previous: vi.fn() };
    const second = { noun: "trace", next: vi.fn(), previous: vi.fn() };
    const takeFirstBack = offerPageSteps(first);
    expect(pageSteps()).toBe(first);

    const takeSecondBack = offerPageSteps(second);
    expect(pageSteps()).toBe(second);
    // The first page's panel going after the second came leaves the second's.
    takeFirstBack();
    expect(pageSteps()).toBe(second);

    takeSecondBack();
    expect(pageSteps()).toBeNull();
  });
});
