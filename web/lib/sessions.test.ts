import { describe, expect, it } from "vitest";

import type { SessionTrace } from "@/lib/server/entities/sessions";

import {
  DEFAULT_SESSION_COLUMNS,
  SESSIONS,
  SESSION_COLUMN_MENU,
  currentSessionSort,
  nextSessionSort,
  orderedSessionColumns,
  parseSessionQuery,
  sessionHref,
  sessionSums,
  sessionsHref,
} from "./sessions";

/** A run of the session, ended at `ended_at`. */
function trace(ended_at: string): SessionTrace {
  return {
    run_id: "5d4f5c68-0000-4000-8000-000000000001",
    agent: "support-bot",
    user_id: "",
    session_id: "conv-1",
    ended: true,
    status: "ok",
    error_type: "",
    started_at: "2026-10-11T03:00:00Z",
    ended_at,
    duration_ms: 0,
    time_to_first_token_ms: null,
    spans: 1,
    steps: 0,
    model_calls: 0,
    tool_calls: 0,
    errors: 0,
    input_tokens: 0,
    output_tokens: 0,
    cache_read_input_tokens: 0,
    cache_write_input_tokens: 0,
    models: [],
    tools: [],
    environments: [],
    releases: [],
    pinned: [],
  };
}

describe("a session's sums", () => {
  it("ends with the run that ended last, as times, though as text a whole second sorts later", () => {
    const sums = sessionSums([trace("2026-10-11T03:00:01.250Z"), trace("2026-10-11T03:00:01Z")]);
    expect(sums.lastEnded).toBe("2026-10-11T03:00:01.250Z");
    expect(sessionSums([]).lastEnded).toBe("");
  });
});

describe("the sessions list's URL", () => {
  it("reads the filters, the sort, the window and the page, and drops what the server would refuse", () => {
    expect(
      parseSessionQuery({
        sort: "session",
        dir: "asc",
        days: "90",
        limit: "100",
        page: "3",
        status: ["error", "gone"],
        user: "user-a",
      }),
    ).toEqual({
      user: ["user-a"],
      status: ["error"],
      sort: "session",
      dir: "asc",
      days: 90,
      limit: 100,
      page: 3,
    });
    expect(parseSessionQuery({ sort: "input_tokens" })).toEqual({ sort: "input_tokens" });
    // Rows a page does not offer, and the first page, which the URL leaves out.
    expect(parseSessionQuery({ limit: "200", page: "1" })).toEqual({});
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
    expect(nextSessionSort({}, "last_seen")).toEqual({
      sort: "last_seen",
      dir: "asc",
      page: undefined,
    });
    expect(nextSessionSort({}, "user")).toEqual({ sort: "user", dir: "asc", page: undefined });
    expect(nextSessionSort({ sort: "tokens", page: 4 }, "tokens")).toEqual({
      sort: "tokens",
      dir: "asc",
      page: undefined,
    });
    expect(nextSessionSort({}, "output_tokens")).toEqual({
      sort: "output_tokens",
      dir: "desc",
      page: undefined,
    });
  });
});

describe("the sessions table's columns", () => {
  it("keeps the columns picked in the table's order, once, and none it does not have", () => {
    expect(orderedSessionColumns(["last_seen", "user", "user", "agent", "session"])).toEqual([
      "user",
      "last_seen",
    ]);
    expect(DEFAULT_SESSION_COLUMNS).toEqual([
      "user",
      "traces",
      "errors",
      "steps",
      "tokens",
      "duration",
      "first_seen",
      "last_seen",
    ]);
  });

  it("files the counts and the tokens each under its own heading in the menu", () => {
    expect(
      SESSION_COLUMN_MENU.map((block) =>
        block.kind === "group" ? `${block.label}: ${block.columns.map((c) => c.key).join()}` : block.column.key,
      ),
    ).toEqual([
      "user",
      "Counts: traces,errors,steps",
      "Tokens: input_tokens,output_tokens,tokens",
      "duration",
      "first_seen",
      "last_seen",
    ]);
  });
});

describe("the sessions list's ready-made queries and saved views", () => {
  it("applies each preset by the list's own sorts, and takes it off again", () => {
    const slow = SESSIONS.presetToggled({ page: 2 }, "slow");
    expect(slow).toEqual({ sort: "duration", dir: "desc" });
    expect(SESSIONS.presetActive(slow, "slow")).toBe(true);
    expect(SESSIONS.presetToggled(slow, "tokens")).toEqual({ sort: "tokens", dir: "desc" });
    expect(SESSIONS.presetToggled(slow, "slow")).toEqual({});
    expect(SESSIONS.presetToggled({}, "errors")).toEqual({ status: ["error"] });
  });

  it("opens a view on its query, named in the URL, and knows when the list has moved off it", () => {
    const view = { id: "8f14e45f-ceea-4e7a-9b6f-1d2c3a4b5c6d", query: { sort: "duration" as const, days: 30 } };
    expect(SESSIONS.viewHref("acme", "web", view)).toBe(
      `/acme/web/sessions?sort=duration&days=30&view=${view.id}`,
    );
    expect(SESSIONS.showsView({ sort: "duration", days: 30, page: 3 }, view)).toBe(true);
    expect(SESSIONS.showsView({ sort: "tokens", days: 30 }, view)).toBe(false);
    expect(SESSIONS.viewQueryOf({ sort: "duration", days: 30, page: 3 })).toEqual(view.query);
  });
});
