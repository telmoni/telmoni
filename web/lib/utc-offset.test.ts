import { describe, expect, it } from "vitest";

import { browserUtcOffset, parseUtcOffset } from "./utc-offset";

describe("the reader's clock, from the cookie the browser leaves", () => {
  it("reads whole minutes east of UTC within the world's offsets", () => {
    expect(parseUtcOffset("330")).toBe(330);
    expect(parseUtcOffset("-300")).toBe(-300);
    expect(parseUtcOffset("840")).toBe(840);
    expect(parseUtcOffset("-720")).toBe(-720);
  });

  it("reads UTC where there is no offset, or none a clock is set to", () => {
    expect(parseUtcOffset(undefined)).toBe(0);
    expect(parseUtcOffset("")).toBe(0);
    expect(parseUtcOffset("841")).toBe(0);
    expect(parseUtcOffset("-721")).toBe(0);
    expect(parseUtcOffset("5.5")).toBe(0);
    expect(parseUtcOffset("1e2")).toBe(0);
    expect(parseUtcOffset("330; path=/")).toBe(0);
  });

  it("gives this browser's offset east of UTC, as the server takes it", () => {
    const date = new Date("2026-10-11T12:00:00Z");
    expect(browserUtcOffset(date)).toBe(-date.getTimezoneOffset() || 0);
  });
});
