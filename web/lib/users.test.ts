import { describe, expect, it } from "vitest";

import {
  DEFAULT_USER_COLUMNS,
  USER_COLUMN_MENU,
  currentUserSort,
  nextUserSort,
  orderedUserColumns,
  parseUserQuery,
  usersHref,
} from "./users";

describe("the users list's URL", () => {
  it("reads the filters, the sort, the window and the page, and drops what the server would refuse", () => {
    expect(
      parseUserQuery({
        sort: "first_seen",
        dir: "desc",
        days: "30",
        limit: "25",
        page: "2",
        model: "claude-sonnet-5-5",
        tag: ["team:search", "no-colon"],
      }),
    ).toEqual({
      model: ["claude-sonnet-5-5"],
      tag: ["team:search"],
      sort: "first_seen",
      dir: "desc",
      days: 30,
      limit: 25,
      page: 2,
    });
    expect(parseUserQuery({ sort: "spend", dir: "sideways", days: "0", limit: "500" })).toEqual({});
    expect(parseUserQuery({ user: "someone" })).toEqual({ user: ["someone"] });
  });

  it("writes the list's path with one thing changed", () => {
    expect(usersHref("acme", "web", {})).toBe("/acme/web/users");
    expect(usersHref("acme", "web", { sort: "traces", days: 30 }, { limit: 100 })).toBe(
      "/acme/web/users?sort=traces&days=30&limit=100",
    );
    expect(usersHref("acme", "web", { sort: "traces", dir: "asc" }, { dir: undefined })).toBe(
      "/acme/web/users?sort=traces",
    );
    expect(usersHref("acme", "web", { status: ["error"], page: 2 }, { page: undefined })).toBe(
      "/acme/web/users?status=error",
    );
  });

  it("sorts most tokens first by default, and each heading its own way from the first page", () => {
    expect(currentUserSort({})).toEqual({ sort: "tokens", dir: "desc" });
    expect(currentUserSort({ sort: "user" })).toEqual({ sort: "user", dir: "asc" });
    expect(nextUserSort({}, "tokens")).toEqual({ sort: "tokens", dir: "asc", page: undefined });
    expect(nextUserSort({}, "first_seen")).toEqual({
      sort: "first_seen",
      dir: "asc",
      page: undefined,
    });
    expect(nextUserSort({ sort: "errors", page: 3 }, "errors")).toEqual({
      sort: "errors",
      dir: "asc",
      page: undefined,
    });
    expect(nextUserSort({ sort: "errors" }, "last_seen")).toEqual({
      sort: "last_seen",
      dir: "desc",
      page: undefined,
    });
    expect(nextUserSort({}, "input_tokens")).toEqual({
      sort: "input_tokens",
      dir: "desc",
      page: undefined,
    });
  });
});

describe("the users table's columns", () => {
  it("keeps the columns picked in the table's order, once, and none it does not have", () => {
    expect(orderedUserColumns(["tokens", "traces", "user", "tokens"])).toEqual(["traces", "tokens"]);
    expect(DEFAULT_USER_COLUMNS).toEqual([
      "traces",
      "errors",
      "steps",
      "tokens",
      "first_seen",
      "last_seen",
    ]);
  });

  it("files the counts and the tokens each under its own heading in the menu", () => {
    expect(
      USER_COLUMN_MENU.map((block) =>
        block.kind === "group" ? `${block.label}: ${block.columns.map((c) => c.key).join()}` : block.column.key,
      ),
    ).toEqual([
      "Counts: traces,errors,steps",
      "Tokens: input_tokens,output_tokens,tokens",
      "first_seen",
      "last_seen",
    ]);
  });
});
