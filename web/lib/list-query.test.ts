import { describe, expect, it } from "vitest";

import { SortDirection, SpanStatus } from "@/lib/types/enums";

import {
  compact,
  listHref,
  one,
  oneOf,
  parseWindow,
  queryString,
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
