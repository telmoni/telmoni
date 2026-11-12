import { describe, expect, it } from "vitest";

import { SessionSort } from "@/lib/types/enums";

import {
  DEFAULT_SESSION_COLUMNS,
  SESSIONS,
  SESSION_COLUMNS,
  SessionColumn,
  parseSessionQuery,
  sessionsHref,
} from "./sessions";

describe("the sessions list", () => {
  it("sorts by every column the server sorts by, each its own way first, and by nothing else", () => {
    expect(SESSION_COLUMNS.flatMap((column) => (column.sort ? [column.sort] : [])).sort()).toEqual(
      Object.values(SessionSort).sort(),
    );
    for (const column of SESSION_COLUMNS) {
      expect(Boolean(column.sort) === Boolean(column.first), column.key).toBe(true);
    }
    expect(SESSION_COLUMNS.map((column) => column.key).sort()).toEqual(Object.values(SessionColumn).sort());
  });

  it("opens on what a session was, took and was scored, and on no user's hash", () => {
    expect(DEFAULT_SESSION_COLUMNS).toEqual([
      "agents",
      "last_seen",
      "traces",
      "duration",
      "tokens",
      "spend",
      "scores",
      "tags",
    ]);
    expect(SESSIONS.ordered(["tags", "price", "agents", "agents"])).toEqual(["agents", "tags"]);
  });

  it("reads its URL as the trace list reads its own, a sort of another list's dropped", () => {
    expect(
      parseSessionQuery({ agent: ["support-bot", "triage"], status: "error", sort: "spend", dir: "asc", days: "30", page: "2" }),
    ).toEqual({ agent: ["support-bot", "triage"], status: ["error"], sort: "spend", dir: "asc", days: 30, page: 2 });
    expect(parseSessionQuery({ sort: "started" })).toEqual({});
    expect(parseSessionQuery({ sort: "tag:repo" })).toEqual({});
    expect(SESSIONS.current({})).toEqual({ sort: "last_seen", dir: "desc" });
    expect(SESSIONS.current({ sort: "first_seen" })).toEqual({ sort: "first_seen", dir: "asc" });
  });

  it("writes its path with one thing changed, and a heading's click from the first page", () => {
    expect(sessionsHref("acme", "web", {})).toBe("/acme/web/sessions");
    expect(sessionsHref("acme", "web", { agent: ["triage"], page: 3 }, { user: ["u1"], page: undefined })).toBe(
      "/acme/web/sessions?agent=triage&user=u1",
    );
    expect(SESSIONS.next({ sort: "spend", page: 4 }, "spend")).toEqual({ sort: "spend", dir: "asc", page: undefined });
    expect(SESSIONS.next({}, "traces")).toEqual({ sort: "traces", dir: "desc", page: undefined });
  });

  it("offers the ready-made queries the trace list does, each its own sort", () => {
    expect(SESSIONS.presets.map((preset) => preset.id)).toEqual(["errors", "slow", "tokens"]);
    expect(SESSIONS.presetToggled({}, "errors")).toEqual({ status: ["error"] });
    expect(SESSIONS.presetToggled({}, "slow")).toEqual({ sort: "duration", dir: "desc" });
    expect(SESSIONS.presetActive({ sort: "tokens" }, "tokens")).toBe(true);
    expect(SESSIONS.presetToggled({ sort: "tokens", dir: "desc" }, "tokens")).toEqual({});
  });
});
