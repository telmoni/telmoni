import { describe, expect, it } from "vitest";

import { SortDirection, SpanStatus } from "@/lib/types/enums";

import {
  FILTER_VALUES_MAX,
  compact,
  isUuid,
  listHref,
  many,
  one,
  oneOf,
  parseWindow,
  queryString,
  sameValues,
  sole,
  sortAfterClick,
  sortInForce,
  whole,
  windowReaching,
} from "./list-query";

describe("a window that reaches a run", () => {
  const now = Date.parse("2026-10-11T12:00:00Z");

  it("names none while the week a list opens on holds it", () => {
    expect(windowReaching("2026-10-11T11:00:00Z", now)).toBeUndefined();
    expect(windowReaching("2026-10-04T12:00:00Z", now)).toBeUndefined();
  });

  it("names the narrowest window offered past that, and the longest kept past those", () => {
    expect(windowReaching("2026-10-03T12:00:00Z", now)).toBe(30);
    expect(windowReaching("2026-08-01T12:00:00Z", now)).toBe(90);
    expect(windowReaching("2026-01-01T00:00:00Z", now)).toBe(360);
  });
});

describe("reading a list's URL", () => {
  it("takes a parameter's first value, trimmed, and none when it is empty", () => {
    expect(one(" nightly-report ")).toBe("nightly-report");
    expect(one(["conv-1", "conv-2"])).toBe("conv-1");
    expect(one("  ")).toBeUndefined();
    expect(one([])).toBeUndefined();
    expect(one(undefined)).toBeUndefined();
  });

  it("reads a whole number within its bound and nothing else", () => {
    expect(whole("30", 360)).toBe(30);
    expect(whole(["1"], 360)).toBe(1);
    expect(whole("0", 360)).toBeUndefined();
    expect(whole("361", 360)).toBeUndefined();
    expect(whole("7.5", 360)).toBeUndefined();
    expect(whole("-1", 360)).toBeUndefined();
    expect(whole("1e2", 360)).toBeUndefined();
    expect(whole(undefined, 360)).toBeUndefined();
  });

  it("reads a filter given again as its values, each trimmed and once", () => {
    expect(many(["support-bot", " zeta ", "support-bot", "  "])).toEqual(["support-bot", "zeta"]);
    expect(many("nightly-report")).toEqual(["nightly-report"]);
    expect(many(undefined)).toEqual([]);
    const lots = Array.from({ length: FILTER_VALUES_MAX + 5 }, (_, i) => `agent-${i}`);
    expect(many(lots)).toHaveLength(FILTER_VALUES_MAX);
  });

  it("reads a word of a vocabulary, and not its name", () => {
    expect(oneOf("error", SpanStatus)).toBe("error");
    expect(oneOf(["ok"], SpanStatus)).toBe("ok");
    expect(oneOf("maybe", SpanStatus)).toBeUndefined();
    expect(oneOf("Ok", SpanStatus)).toBeUndefined();
    expect(oneOf(undefined, SpanStatus)).toBeUndefined();
  });

  it("reads the window, dropping what the server would refuse", () => {
    expect(parseWindow({ days: "30", limit: "500" })).toEqual({ days: 30, limit: 500 });
    expect(parseWindow({ days: "0", limit: "9999" })).toEqual({});
    expect(parseWindow({})).toEqual({});
  });

  it("compacts a query to the keys that are set", () => {
    const query = compact({ sort: "user", dir: undefined, days: 30 });
    expect(query).toEqual({ sort: "user", days: 30 });
    expect("dir" in query).toBe(false);
  });
});

describe("writing it", () => {
  it("carries only the keys that are set, as a lane or a path takes them", () => {
    expect(queryString({})).toBe("");
    expect(queryString({ agent: "", user: undefined, days: 30, status: "error" })).toBe(
      "?days=30&status=error",
    );
    expect(queryString({ user: "a b&c" })).toBe("?user=a+b%26c");
    expect(queryString({ agent: ["support-bot", "", "zeta"], status: [] })).toBe(
      "?agent=support-bot&agent=zeta",
    );
  });

  it("patches a list's path, dropping a key patched away and keeping the rest in place", () => {
    expect(listHref("acme", "web", "/users", {})).toBe("/acme/web/users");
    expect(
      listHref("acme", "web", "/users", { sort: "user", days: 30 }, { days: undefined, dir: "desc" }),
    ).toBe("/acme/web/users?sort=user&dir=desc");
    expect(listHref("acme", "web", "/sessions/chat%2F42", { limit: 500 })).toBe(
      "/acme/web/sessions/chat%2F42?limit=500",
    );
  });
});

describe("sorting by a heading", () => {
  const columns = [
    { sort: "name", label: "Name", first: SortDirection.Asc },
    { sort: "tokens", label: "Tokens", first: SortDirection.Desc },
  ] as const;

  it("takes the URL's sort, else the default, each column its own way", () => {
    expect(sortInForce(columns, "tokens", {})).toEqual({ sort: "tokens", dir: "desc" });
    expect(sortInForce(columns, "tokens", { sort: "name" })).toEqual({ sort: "name", dir: "asc" });
    expect(sortInForce(columns, "tokens", { sort: "name", dir: SortDirection.Desc })).toEqual({
      sort: "name",
      dir: "desc",
    });
  });

  it("flips the column in force on a second click, and starts another its own way", () => {
    expect(sortAfterClick(columns, "tokens", {}, "tokens")).toEqual({ sort: "tokens", dir: "asc" });
    expect(sortAfterClick(columns, "tokens", {}, "name")).toEqual({ sort: "name", dir: "asc" });
    expect(sortAfterClick(columns, "tokens", { sort: "name" }, "name")).toEqual({
      sort: "name",
      dir: "desc",
    });
    expect(sortAfterClick(columns, "tokens", { sort: "name", dir: SortDirection.Desc }, "tokens")).toEqual({
      sort: "tokens",
      dir: "desc",
    });
  });
});

describe("sole", () => {
  it("takes one string and nothing else, a repeated parameter among the nothing", () => {
    expect(sole("x")).toBe("x");
    expect(sole("")).toBeUndefined();
    expect(sole(["x", "y"])).toBeUndefined();
    expect(sole(undefined)).toBeUndefined();
  });
});

describe("sameValues", () => {
  it("holds two lists the same in any order, and none the same as none", () => {
    expect(sameValues(["a", "b"], ["b", "a"])).toBe(true);
    expect(sameValues(["a"], ["a", "a"])).toBe(false);
    expect(sameValues(undefined, [])).toBe(true);
  });
});

describe("isUuid", () => {
  it("knows a UUID of any version, and nothing shorter or wider", () => {
    expect(isUuid("7bQx2mNv9BcK4dLp")).toBe(false);
    expect(isUuid("0192f4b2-8c3a-7d1e-9f00-123456789abc")).toBe(true);
    expect(isUuid("0192F4B2-8C3A-4D1E-9F00-123456789ABC")).toBe(true);
    expect(isUuid("0192f4b2-8c3a-7d1e-9f00-123456789ab")).toBe(false);
  });
});
