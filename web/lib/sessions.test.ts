import { describe, expect, it } from "vitest";

import type { SessionTrace } from "@/lib/server/entities/sessions";

import {
  currentSessionSort,
  nextSessionSort,
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
