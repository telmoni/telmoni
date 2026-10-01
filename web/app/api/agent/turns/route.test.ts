// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from "vitest";

const {
  fetchWithTimeout,
  getSession,
  isSessionBlacklisted,
  rateLimit,
  identityContext,
  fetchProject,
} = vi.hoisted(() => ({
  fetchWithTimeout: vi.fn(),
  getSession: vi.fn(),
  isSessionBlacklisted: vi.fn(async () => false),
  rateLimit: vi.fn(async (): Promise<Response | null> => null),
  identityContext: vi.fn(),
  fetchProject: vi.fn(),
}));

vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit,
  sessionKey: (session: { userId: string }, action: string) => `bfrl:${session.userId}:${action}`,
}));
vi.mock("@/lib/auth/session", () => ({ getSession }));
vi.mock("@/lib/auth/session-blacklist", () => ({ isSessionBlacklisted }));
vi.mock("@/lib/env", () => ({
  env: { SERVER_URL: "http://server:8082", SERVICE_SECRET: "s3cret" },
}));
// The real `projectHeaders`: which headers reach the server is the thing
// under test, so only who is asking and which projects they have are stubbed.
vi.mock("@/lib/server/entities/identity-context", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/identity-context")>()),
  identityContext,
}));
vi.mock("@/lib/server/entities/projects", () => ({ fetchProject }));

import { NextRequest } from "next/server";

import { AGENT_STREAM_TIMEOUT_MS } from "@/lib/server/entities/agent";

import { POST } from "./route";

const PROJECT = "project_abc";
const CONVERSATION = "0192a3b4-c5d6-7e8f-9a0b-1c2d3e4f5a6b";

function turn(body: unknown, headers: Record<string, string> = {}) {
  return POST(
    new NextRequest("https://app.example/api/agent/turns", {
      method: "POST",
      headers: { "content-type": "application/json", "sec-fetch-site": "same-origin", ...headers },
      body: typeof body === "string" ? body : JSON.stringify(body),
    }),
  );
}

function sent(): { url: string; init: RequestInit; headers: Headers; ms: number } {
  const [url, init, ms] = fetchWithTimeout.mock.calls[0]!;
  return { url, init, headers: new Headers(init.headers), ms };
}

function sse(text: string): Response {
  return new Response(
    new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(new TextEncoder().encode(text));
        c.close();
      },
    }),
    { status: 200, headers: { "content-type": "text/event-stream" } },
  );
}

beforeEach(() => {
  fetchWithTimeout.mockReset().mockResolvedValue(sse('event: text\ndata: {"delta":"hi"}\n\n'));
  getSession.mockReset().mockResolvedValue({ userId: "user_1", sessionRowId: "sess_1" });
  isSessionBlacklisted.mockReset().mockResolvedValue(false);
  rateLimit.mockReset().mockResolvedValue(null);
  identityContext.mockReset().mockResolvedValue({
    userId: "user_1",
    organizationId: "org_1",
    role: "member",
    accessToken: "at_person",
  });
  fetchProject.mockReset().mockResolvedValue({ id: PROJECT });
});

describe("POST /api/agent/turns", () => {
  it("refuses a cross-site request before it reads the session", async () => {
    const res = await turn({ projectId: PROJECT, message: "hi" }, { "sec-fetch-site": "cross-site" });
    expect(res.status).toBe(403);
    expect(getSession).not.toHaveBeenCalled();
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("answers 401 JSON without a session", async () => {
    getSession.mockResolvedValue(null);
    const res = await turn({ projectId: PROJECT, message: "hi" });
    expect(res.status).toBe(401);
    expect(await res.json()).toEqual({ error: "unauthenticated" });
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("answers 401 for a revoked session", async () => {
    isSessionBlacklisted.mockResolvedValue(true);
    const res = await turn({ projectId: PROJECT, message: "hi" });
    expect(res.status).toBe(401);
  });

  it.each([
    ["not JSON", "{"],
    ["no project", { message: "hi" }],
    ["a malformed project", { projectId: "../x", message: "hi" }],
    ["a blank message", { projectId: PROJECT, message: "   " }],
    ["a message over the ceiling", { projectId: PROJECT, message: "x".repeat(4001) }],
    ["a conversation that is not a uuid", { projectId: PROJECT, conversationId: "c1", message: "hi" }],
  ])("refuses %s with 400", async (_name, body) => {
    const res = await turn(body);
    expect(res.status).toBe(400);
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("refuses a project the person does not have", async () => {
    fetchProject.mockResolvedValue(null);
    const res = await turn({ projectId: PROJECT, message: "hi" });
    expect(res.status).toBe(404);
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("sends the person's bearer and the project lane's headers, with the trimmed turn", async () => {
    await turn({ projectId: PROJECT, conversationId: CONVERSATION, message: "  who is here?  " });
    const { url, init, headers, ms } = sent();
    expect(url).toBe("http://server:8082/internal/agent/turns");
    expect(init.method).toBe("POST");
    expect(ms).toBe(AGENT_STREAM_TIMEOUT_MS);
    expect(headers.get("authorization")).toBe("Bearer at_person");
    expect(headers.get("x-service-secret")).toBe("s3cret");
    expect(headers.get("x-request-id")).toMatch(/^[0-9a-f-]{36}$/);
    expect(headers.get("x-organization-id")).toBe("org_1");
    expect(headers.get("x-project-id")).toBe(PROJECT);
    expect(JSON.parse(init.body as string)).toEqual({
      conversation_id: CONVERSATION,
      message: "who is here?",
    });
  });

  it("sends a new conversation as a null id", async () => {
    await turn({ projectId: PROJECT, message: "hi" });
    expect(JSON.parse(sent().init.body as string)).toEqual({
      conversation_id: null,
      message: "hi",
    });
  });

  it("passes the server's stream through with the event-stream headers", async () => {
    const res = await turn({ projectId: PROJECT, message: "hi" });
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("text/event-stream; charset=utf-8");
    expect(res.headers.get("cache-control")).toBe("no-cache, no-transform");
    expect(res.headers.get("x-accel-buffering")).toBe("no");
    expect(await res.text()).toBe('event: text\ndata: {"delta":"hi"}\n\n');
  });

  it("passes a refusal through as the problem the server answered", async () => {
    const problem = {
      type: "/errors/agent/rate-limited",
      title: "rate limited",
      status: 429,
      retry_after_secs: 30,
    };
    fetchWithTimeout.mockResolvedValue(
      new Response(JSON.stringify(problem), {
        status: 429,
        headers: { "content-type": "application/problem+json", "retry-after": "30" },
      }),
    );
    const res = await turn({ projectId: PROJECT, message: "hi" });
    expect(res.status).toBe(429);
    expect(res.headers.get("content-type")).toBe("application/problem+json");
    expect(res.headers.get("retry-after")).toBe("30");
    expect(await res.json()).toEqual(problem);
  });

  it("does not pass on a refusal that is not JSON", async () => {
    fetchWithTimeout.mockResolvedValue(
      new Response("<html>bad gateway</html>", {
        status: 502,
        headers: { "content-type": "text/html" },
      }),
    );
    const res = await turn({ projectId: PROJECT, message: "hi" });
    expect(res.status).toBe(502);
    expect(await res.json()).toEqual({ title: "upstream error", status: 502 });
  });

  it("answers 503 when the server cannot be reached", async () => {
    fetchWithTimeout.mockRejectedValue(new TypeError("fetch failed"));
    const res = await turn({ projectId: PROJECT, message: "hi" });
    expect(res.status).toBe(503);
  });

  it("stops at the console's own ceiling", async () => {
    rateLimit.mockResolvedValue(new Response(null, { status: 429 }));
    const res = await turn({ projectId: PROJECT, message: "hi" });
    expect(res.status).toBe(429);
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });
});
