import { describe, it, expect } from "vitest";
import { cn } from "./utils";

describe("cn", () => {
  // ⚠ **Only the value tailwind-merge does NOT ship belongs here.** It
  // resolves `rounded-*` by matching known values, so an unrecognised one is
  // not a radius to it: it keeps both classes and lets CSS source order pick
  // the winner instead of the order they were written. Nothing about that is
  // visible at the callsite — the class is spelled right, the token is real,
  // and the element takes the other one's radius.
  // A case built from stock values (`full`, `none`, any step) would pass with
  // this config deleted, which is why none appears on the left AND right.
  it.each([
    ["rounded-md", "rounded-menu"],
    ["rounded-menu", "rounded-md"],
  ])("lets %s lose to %s like any other radius", (base, override) => {
    expect(cn(base, override)).toBe(override);
  });

  // The stock values still have to conflict with each other. An `extend` that
  // replaced the scale rather than adding to it would leave every one of them
  // unrecognised, and the cases above would not notice.
  it("leaves the stock radius values arbitrating normally", () => {
    expect(cn("rounded-sm", "rounded-lg")).toBe("rounded-lg");
    expect(cn("rounded-full", "rounded-md")).toBe("rounded-md");
    expect(cn("rounded-md", "rounded-full")).toBe("rounded-full");
    expect(cn("rounded-menu", "rounded-none")).toBe("rounded-none");
  });
});
