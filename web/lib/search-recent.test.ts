// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";

import {
  RECENT_LIMIT,
  clearRecentForTest,
  readRecent,
  recordVisit,
} from "./search-recent";

const KEY = "telmoni-recent-visits";

beforeEach(() => {
  window.localStorage.clear();
  clearRecentForTest();
});

describe("the recent-visit trail", () => {
  it("is empty before anything is visited", () => {
    expect(readRecent()).toEqual([]);
  });

  it("puts the newest first", () => {
    recordVisit({ href: "/t/api-keys", label: "API keys" });
    recordVisit({ href: "/t/members", label: "Members" });
    expect(readRecent().map((v) => v.label)).toEqual(["Members", "API keys"]);
  });

  it("moves a revisited page up instead of listing it twice", () => {
    recordVisit({ href: "/t/api-keys", label: "API keys" });
    recordVisit({ href: "/t/members", label: "Members" });
    recordVisit({ href: "/t/api-keys", label: "API keys" });

    const trail = readRecent();
    expect(trail.map((v) => v.label)).toEqual(["API keys", "Members"]);
    expect(trail).toHaveLength(2);
  });

  it(`keeps ${RECENT_LIMIT} and drops the oldest`, () => {
    for (let i = 0; i < RECENT_LIMIT + 3; i++) {
      recordVisit({ href: `/t/p${i}`, label: `Page ${i}` });
    }
    const trail = readRecent();
    expect(trail).toHaveLength(RECENT_LIMIT);
    expect(trail[0]?.label).toBe(`Page ${RECENT_LIMIT + 2}`);
    expect(trail.at(-1)?.label).toBe(`Page 3`);
  });

  describe("a trail it cannot read", () => {
    it.each([
      ["text that is not JSON", "{not json"],
      ["JSON that is not an array", '{"href":"/x"}'],
      ["rows missing their fields", '[{"href":"/x"}]'],
      ["a row whose href is not a string", '[{"href":1,"label":"x","at":0}]'],
    ])("drops %s rather than half-trusting it", (_, raw) => {
      window.localStorage.setItem(KEY, raw);
      expect(readRecent()).toEqual([]);
    });
  });

  describe("when storage refuses", () => {
    function withBlockedStorage(body: () => void) {
      const real = Object.getOwnPropertyDescriptor(window, "localStorage")!;
      Object.defineProperty(window, "localStorage", {
        configurable: true,
        get() {
          throw new DOMException("denied", "SecurityError");
        },
      });
      try {
        body();
      } finally {
        Object.defineProperty(window, "localStorage", real);
      }
    }

    it("still remembers the visits made in this session", () => {
      withBlockedStorage(() => {
        expect(() => recordVisit({ href: "/t/api-keys", label: "API keys" })).not.toThrow();
        expect(readRecent().map((v) => v.label)).toEqual(["API keys"]);
      });
    });

    it("reads as empty rather than throwing out of a render", () => {
      withBlockedStorage(() => {
        expect(() => readRecent()).not.toThrow();
        expect(readRecent()).toEqual([]);
      });
    });
  });
});
