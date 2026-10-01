import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

// Comments stripped: this file's own prose explains why a hook does NOT call
// `getState()`, and a scan that reads prose finds the word it is banning.
const SRC = readFileSync(path.join(__dirname, "index.ts"), "utf8").replace(
  /\/\*[\s\S]*?\*\/|\/\/[^\n]*/g,
  "",
);

describe("the store's hooks", () => {
  // ⚠ **zustand v5 dropped the equality argument, so an atomic selector is the
  // whole strategy.** A selector that builds an object or an array hands back a
  // new reference every call, `useSyncExternalStore` compares with `Object.is`,
  // and the consumer then re-renders on every unrelated write — the version
  // that loops is the one that also derives during render. `useShallow` is the
  // only sanctioned way to take more than one value at a time.
  it("selects one value at a time, unless it reaches for useShallow", () => {
    const calls = [
      ...SRC.matchAll(
        /useStore\(\s*(useShallow\()?\s*\(\w+\) =>\s*([\s\S]{0,12})/g,
      ),
    ];
    expect(calls.length, "selectors to check").toBeGreaterThanOrEqual(10);
    for (const [, shallow, body] of calls) {
      if (shallow) continue;
      const opens = body!.trimStart()[0]!;
      expect(
        ["{", "["].includes(opens),
        `a bare selector opens with ${opens}, so it builds a fresh reference every call`,
      ).toBe(false);
    }
  });

  // The whole point of routing reads through `useSyncExternalStore`: a
  // component body that calls `getState()` reads a mutable store outside
  // React's knowledge, which is what tears under concurrent rendering.
  it("never reads the store's state during a render", () => {
    expect(SRC, "a hook reaches for getState()").not.toMatch(/getState\(\)/);
  });
});
