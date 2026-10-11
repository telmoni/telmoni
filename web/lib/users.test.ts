import { describe, expect, it } from "vitest";

import { currentUserSort, nextUserSort, parseUserQuery, usersHref } from "./users";

describe("the users list's URL", () => {
  it("reads the sort and the window, and drops what the server would refuse", () => {
    expect(parseUserQuery({ sort: "first_seen", dir: "desc", days: "30", limit: "500" })).toEqual({
      sort: "first_seen",
      dir: "desc",
      days: 30,
      limit: 500,
    });
    expect(parseUserQuery({ sort: "spend", dir: "sideways", days: "0", limit: "9999" })).toEqual({});
    expect(parseUserQuery({ user: "someone" })).toEqual({});
  });

  it("writes the list's path with one thing changed", () => {
    expect(usersHref("acme", "web", {})).toBe("/acme/web/users");
    expect(usersHref("acme", "web", { sort: "traces", days: 30 }, { limit: 500 })).toBe(
      "/acme/web/users?sort=traces&days=30&limit=500",
    );
    expect(usersHref("acme", "web", { sort: "traces", dir: "asc" }, { dir: undefined })).toBe(
      "/acme/web/users?sort=traces",
    );
  });

  it("sorts most tokens first by default, and each heading its own way", () => {
    expect(currentUserSort({})).toEqual({ sort: "tokens", dir: "desc" });
    expect(currentUserSort({ sort: "user" })).toEqual({ sort: "user", dir: "asc" });
    expect(nextUserSort({}, "tokens")).toEqual({ sort: "tokens", dir: "asc" });
    expect(nextUserSort({}, "first_seen")).toEqual({ sort: "first_seen", dir: "asc" });
    expect(nextUserSort({ sort: "errors" }, "errors")).toEqual({ sort: "errors", dir: "asc" });
    expect(nextUserSort({ sort: "errors" }, "last_seen")).toEqual({ sort: "last_seen", dir: "desc" });
  });
});
