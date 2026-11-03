import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimitRetryAfter: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/entities/identity-context", () => ({
  identityContext: vi.fn(async () => ({ accessToken: "at_1", organizationId: "org_1" })),
  projectHeaders: vi.fn(() => ({ authorization: "Bearer at_1" })),
}));
vi.mock("@/lib/server/entities/projects", () => ({
  fetchProjectAnywhere: vi.fn(async () => ({ id: PROJECT, organizationId: "org_1" })),
}));
vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout: vi.fn() }));
vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://server.test" } }));

import { fetchWithTimeout } from "@/lib/api/fetch";
import { getServerSession } from "@/lib/server/session";
import { TRACE_QUESTION_MAX } from "@/lib/types/agent";

import { askTraceListAction } from "./agent-actions";

const PROJECT = "project_7bQx2mNv9BcK4dLp";

function answered(status: number, body: unknown) {
  vi.mocked(fetchWithTimeout).mockResolvedValueOnce(
    new Response(JSON.stringify(body), {
      status,
      headers: { "content-type": status < 400 ? "application/json" : "application/problem+json" },
    }),
  );
}

describe("askTraceListAction", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ accessToken: "at_1" } as never);
  });

  it("asks the agent's lane the trimmed sentence over the list's window, and hands back its answer", async () => {
    const answer = { query: { status: ["error"], days: 7 }, unmet: null, dropped: [] };
    answered(200, answer);
    expect(await askTraceListAction(PROJECT, "  errors  ", 7)).toEqual({ ok: true, answer });
    const [url, init] = vi.mocked(fetchWithTimeout).mock.calls[0] ?? [];
    expect(url).toMatch(/\/internal\/v1\/agent\/trace-filters$/);
    expect(JSON.parse(String(init?.body))).toEqual({ sentence: "errors", days: 7 });
  });

  it("refuses a blank or long sentence, an odd window and an unknown project, asking nothing", async () => {
    for (const [sentence, days] of [
      ["   ", 7],
      ["x".repeat(TRACE_QUESTION_MAX + 1), 7],
      ["errors", 0],
      ["errors", 1.5],
      ["errors", 361],
    ] as const) {
      expect((await askTraceListAction(PROJECT, sentence, days)).ok).toBe(false);
    }
    expect((await askTraceListAction("../members", "errors", 7)).ok).toBe(false);
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("words the agent's refusals as the panel does, and an answer of the wrong shape as an outage", async () => {
    answered(429, {
      type: "/errors/agent/rate-limited",
      title: "agent rate limited",
      status: 429,
      retry_after_secs: 90,
    });
    expect(await askTraceListAction(PROJECT, "errors", 7)).toEqual({
      ok: false,
      error: "You've sent a lot of messages in a short time. Try again in 90 seconds.",
    });
    answered(402, {
      type: "/errors/plan/limit-reached",
      title: "plan limit reached",
      status: 402,
      detail: "Move to Pro for more questions.",
    });
    expect(await askTraceListAction(PROJECT, "errors", 7)).toEqual({
      ok: false,
      error: "Move to Pro for more questions.",
    });
    answered(404, { type: "/errors/auth/not-found", title: "not found", status: 404 });
    expect(await askTraceListAction(PROJECT, "errors", 7)).toEqual({
      ok: false,
      error: "This project is no longer there.",
    });
    answered(200, { query: "everything" });
    expect(await askTraceListAction(PROJECT, "errors", 7)).toEqual({
      ok: false,
      error: "The agent couldn't answer — try again shortly.",
    });
  });
});
