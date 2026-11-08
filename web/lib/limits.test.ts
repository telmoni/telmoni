import { describe, expect, it } from "vitest";

import { CAP_FIELDS, capTyped } from "./limits";

const field = (key: string) => CAP_FIELDS.find((f) => f.key === key)!;

describe("a cap as typed", () => {
  it("shares the pool when empty, and refuses what is no number or below none", () => {
    expect(capTyped(field("alerts"), "  ")).toBeNull();
    expect(capTyped(field("alerts"), "x")).toEqual({
      refused: "Alerts: a number, none or more, or empty to share the pool.",
    });
    expect(capTyped(field("alerts"), "-1")).toHaveProperty("refused");
  });

  // A count rounded quietly would save a cap nobody typed.
  it("takes a count whole, never rounded", () => {
    expect(capTyped(field("alerts"), "3")).toBe(3);
    expect(capTyped(field("alerts"), "2.5")).toEqual({ refused: "Alerts: a whole number." });
    expect(capTyped(field("spans"), "1.5")).toEqual({ refused: "Spans a month: a whole number." });
  });

  it("takes content in MB to the byte, a fraction among it", () => {
    expect(capTyped(field("content_bytes"), "2.5")).toBe(2_500_000);
  });

  // Past a 32-bit count the server's column refuses it; said here instead.
  it("refuses a count past what the server's column holds, and not spans", () => {
    expect(capTyped(field("alerts"), "2147483647")).toBe(2_147_483_647);
    expect(capTyped(field("alerts"), "2147483648")).toEqual({
      refused: "Alerts: at most 2,147,483,647.",
    });
    expect(capTyped(field("spans"), "5000000000")).toBe(5_000_000_000);
  });
});
