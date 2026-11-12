import { describe, expect, it } from "vitest";

import { UserSort } from "@/lib/types/enums";

import { SessionColumn, SummedColumn, sessionHref } from "./sessions";
import {
  DEFAULT_USER_COLUMNS,
  USERS,
  USER_COLUMNS,
  UserColumn,
  parseUserQuery,
  userHref,
  usersHref,
} from "./users";

describe("the users list", () => {
  it("sorts by every column the server sorts by, each its own way first, and by nothing else", () => {
    expect(USER_COLUMNS.flatMap((column) => (column.sort ? [column.sort] : [])).sort()).toEqual(
      Object.values(UserSort).sort(),
    );
    for (const column of USER_COLUMNS) {
      expect(Boolean(column.sort) === Boolean(column.first), column.key).toBe(true);
    }
    expect(USER_COLUMNS.map((column) => column.key).sort()).toEqual(Object.values(UserColumn).sort());
  });

  it("shares with the sessions list every column that sums a row's traces, under the same words", () => {
    for (const column of Object.values(SummedColumn)) {
      expect(Object.values(UserColumn)).toContain(column);
      expect(Object.values(SessionColumn)).toContain(column);
    }
    expect(Object.values(UserColumn).filter((c) => !Object.values<string>(SummedColumn).includes(c))).toEqual([
      "sessions",
    ]);
  });

  it("opens on who was served, how much and how it went", () => {
    expect(DEFAULT_USER_COLUMNS).toEqual([
      "agents",
      "last_seen",
      "sessions",
      "traces",
      "failed_traces",
      "tokens",
      "spend",
    ]);
    expect(USERS.ordered(["tags", "scores", "agents", "agents"])).toEqual(["agents", "tags"]);
  });

  it("reads its URL as the trace list reads its own, a sort of another list's dropped", () => {
    expect(
      parseUserQuery({ agent: ["support-bot", "triage"], status: "error", sort: "sessions", dir: "asc", days: "30", page: "2" }),
    ).toEqual({ agent: ["support-bot", "triage"], status: ["error"], sort: "sessions", dir: "asc", days: 30, page: 2 });
    expect(parseUserQuery({ sort: "started" })).toEqual({});
    expect(parseUserQuery({ sort: "tag:repo" })).toEqual({});
    expect(USERS.current({})).toEqual({ sort: "last_seen", dir: "desc" });
    expect(USERS.current({ sort: "first_seen" })).toEqual({ sort: "first_seen", dir: "asc" });
  });

  it("writes its path with one thing changed, and a heading's click from the first page", () => {
    expect(usersHref("acme", "web", {})).toBe("/acme/web/users");
    expect(usersHref("acme", "web", { agent: ["triage"], page: 3 }, { tag: ["repo:telmoni"], page: undefined })).toBe(
      "/acme/web/users?agent=triage&tag=repo%3Atelmoni",
    );
    expect(USERS.next({ sort: "sessions", page: 4 }, "sessions")).toEqual({
      sort: "sessions",
      dir: "asc",
      page: undefined,
    });
    expect(USERS.presetToggled({}, "slow")).toEqual({ sort: "duration", dir: "desc" });
  });
});

describe("a user's page and a session's", () => {
  it("are each a path of their own under their list, the id as stored and the window where one is asked", () => {
    expect(userHref("acme", "web", "2c26b46b68ffc68ff99b453c1d304134")).toBe(
      "/acme/web/users/2c26b46b68ffc68ff99b453c1d304134",
    );
    expect(userHref("acme", "web", "2c26b46b", 30)).toBe("/acme/web/users/2c26b46b?days=30");
    expect(sessionHref("acme", "web", "9f86d081", 30)).toBe("/acme/web/sessions/9f86d081?days=30");
    expect(sessionHref("acme", "web", "9f86d081")).toBe("/acme/web/sessions/9f86d081");
  });

  it("keep an id that is no hash within its own segment", () => {
    expect(sessionHref("acme", "web", "a/b?c#d")).toBe("/acme/web/sessions/a%2Fb%3Fc%23d");
    expect(userHref("acme", "web", "a b", 7)).toBe("/acme/web/users/a%20b?days=7");
  });
});
