import { describe, expect, it } from "vitest";

import {
  currentSessionSort,
  nextSessionSort,
  parseSessionQuery,
  sessionHref,
  sessionsHref,
} from "./sessions";

describe("the sessions list's URL", () => {
  it("reads the sort and the window, and drops what the server would refuse", () => {
    expect(parseSessionQuery({ sort: "session", dir: "asc", days: "90", limit: "200" })).toEqual({
      sort: "session",
      dir: "asc",
      days: 90,
      limit: 200,
    });
    expect(parseSessionQuery({ sort: "agent", dir: "up", days: "361", limit: "0" })).toEqual({});
    // The word the page no longer answers to is dropped like any other.
    expect(parseSessionQuery({ sort: "conversation" })).toEqual({});
  });

  it("writes the list's path with one thing changed, and a session's own", () => {
    expect(sessionsHref("acme", "web", {})).toBe("/acme/web/sessions");
    expect(sessionsHref("acme", "web", { sort: "user", days: 30 }, { dir: "desc" })).toBe(
      "/acme/web/sessions?sort=user&days=30&dir=desc",
    );
    expect(sessionHref("acme", "web", "conv-1")).toBe("/acme/web/sessions/conv-1");
    // An id of the application's choosing is encoded whole, slash and all.
    expect(sessionHref("acme", "web", "chat/42 b")).toBe("/acme/web/sessions/chat%2F42%20b");
    expect(sessionHref("acme", "web", "100%", { days: 30, limit: 500 })).toBe(
      "/acme/web/sessions/100%25?days=30&limit=500",
    );
  });

  it("sorts the session last heard from first by default, and each heading its own way", () => {
    expect(currentSessionSort({})).toEqual({ sort: "last_seen", dir: "desc" });
    expect(currentSessionSort({ sort: "session" })).toEqual({
      sort: "session",
      dir: "asc",
    });
    expect(nextSessionSort({}, "last_seen")).toEqual({ sort: "last_seen", dir: "asc" });
    expect(nextSessionSort({}, "user")).toEqual({ sort: "user", dir: "asc" });
    expect(nextSessionSort({ sort: "tokens" }, "tokens")).toEqual({
      sort: "tokens",
      dir: "asc",
    });
  });
});
