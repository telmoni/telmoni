// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";

import {
  RECENT_LIMIT,
  forgetRecent,
  readRecent,
  recordVisit,
} from "./search-recent";

const KEY = "telmoni-recent-visits";
const ME = "user_1";

beforeEach(() => {
  window.localStorage.clear();
  forgetRecent();
});

describe("the recent-visit trail", () => {
  it("is empty before anything is visited", () => {
    expect(readRecent(ME)).toEqual([]);
  });

  it("puts the newest first", () => {
    recordVisit(ME, { href: "/t/api-keys", label: "API keys" });
    recordVisit(ME, { href: "/t/members", label: "Members" });
    expect(readRecent(ME).map((v) => v.label)).toEqual(["Members", "API keys"]);
  });

  it("moves a revisited page up instead of listing it twice", () => {
    recordVisit(ME, { href: "/t/api-keys", label: "API keys" });
    recordVisit(ME, { href: "/t/members", label: "Members" });
    recordVisit(ME, { href: "/t/api-keys", label: "API keys" });

    const trail = readRecent(ME);
    expect(trail.map((v) => v.label)).toEqual(["API keys", "Members"]);
    expect(trail).toHaveLength(2);
  });

  it(`keeps ${RECENT_LIMIT} and drops the oldest`, () => {
    for (let i = 0; i < RECENT_LIMIT + 3; i++) {
      recordVisit(ME, { href: `/t/p${i}`, label: `Page ${i}` });
    }
    const trail = readRecent(ME);
    expect(trail).toHaveLength(RECENT_LIMIT);
    expect(trail[0]?.label).toBe(`Page ${RECENT_LIMIT + 2}`);
    expect(trail.at(-1)?.label).toBe(`Page 3`);
  });

  describe("another person's trail", () => {
    it("is never shown", () => {
      recordVisit(ME, { href: "/t/members", label: "Members" });
      expect(readRecent("user_2")).toEqual([]);
    });

    it("is replaced by the next person's first visit, not joined", () => {
      recordVisit(ME, { href: "/t/members", label: "Members" });
      recordVisit("user_2", { href: "/u/api-keys", label: "API keys" });

      expect(readRecent("user_2").map((v) => v.label)).toEqual(["API keys"]);
      expect(readRecent(ME)).toEqual([]);
    });
  });

  it("is forgotten whole, storage and all", () => {
    recordVisit(ME, { href: "/t/members", label: "Members" });
    forgetRecent();

    expect(readRecent(ME)).toEqual([]);
    expect(window.localStorage.getItem(KEY)).toBeNull();
  });

  describe("a link that leaves the console", () => {
    it.each([
      ["another origin", "https://evil.example/"],
      ["a protocol-relative one", "//evil.example/"],
      ["a backslash the parser reads as a slash", "/\\evil.example/"],
      ["a tab the parser drops", "/\t/evil.example/"],
      ["a script", "javascript:alert(1)"],
    ])("is never read back: %s", (_, href) => {
      window.localStorage.setItem(
        KEY,
        JSON.stringify({ owner: ME, visits: [{ href, label: "Members", at: 0 }] }),
      );
      expect(readRecent(ME)).toEqual([]);
    });

    it("is never recorded", () => {
      recordVisit(ME, { href: "//evil.example/", label: "Members" });
      expect(readRecent(ME)).toEqual([]);
      expect(window.localStorage.getItem(KEY)).toBeNull();
    });
  });

  describe("a trail it cannot read", () => {
    it.each([
      ["text that is not JSON", "{not json"],
      ["a trail with no owner", '[{"href":"/x","label":"x","at":0}]'],
      ["visits missing their fields", '{"owner":"user_1","visits":[{"href":"/x"}]}'],
      ["a visit whose href is not a string", '{"owner":"user_1","visits":[{"href":1,"label":"x","at":0}]}'],
    ])("drops %s rather than half-trusting it", (_, raw) => {
      window.localStorage.setItem(KEY, raw);
      expect(readRecent(ME)).toEqual([]);
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

    it("still remembers the visits made in this session, for their person alone", () => {
      withBlockedStorage(() => {
        expect(() => recordVisit(ME, { href: "/t/api-keys", label: "API keys" })).not.toThrow();
        expect(readRecent(ME).map((v) => v.label)).toEqual(["API keys"]);
        expect(readRecent("user_2")).toEqual([]);
      });
    });

    it("reads as empty rather than throwing out of a render", () => {
      withBlockedStorage(() => {
        expect(() => readRecent(ME)).not.toThrow();
        expect(readRecent(ME)).toEqual([]);
      });
    });
  });
});
