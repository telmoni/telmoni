import { describe, it, expect } from "vitest";
import { cn } from "./utils";

describe("cn", () => {
  // ⚠ **Only the value tailwind-merge does NOT ship belongs here.** It
  // resolves `rounded-*` and spacing by matching known values, so an
  // unrecognised one is neither to it: it keeps both classes and lets CSS
  // source order pick the winner instead of the order they were written.
  // Nothing about that is visible at the callsite — the class is spelled
  // right, the token is real, and the element takes the other one's value.
  // A case built from stock values (`full`, `none`, any step) would pass with
  // this config deleted, which is why none appears on the left AND right.
  it.each([
    ["rounded-md", "rounded-menu"],
    ["rounded-menu", "rounded-md"],
    ["p-gutter", "p-0"],
    ["px-2", "px-gutter"],
    ["gap-gutter", "gap-1"],
  ])("lets %s lose to %s like any other value", (base, override) => {
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
    expect(cn("p-3", "p-2")).toBe("p-2");
  });
});
