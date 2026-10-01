// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from "vitest";

const {
  tryFetchWithTimeout,
  getSession,
  sealConnect,
  identityContext,
  fetchProject,
  projectHeaders,
  rateLimitRetryAfter,
} =
  vi.hoisted(() => ({
    tryFetchWithTimeout: vi.fn(),
    getSession: vi.fn(),
    rateLimitRetryAfter: vi.fn(async (): Promise<number | null> => null),
    sealConnect: vi.fn(async (v: unknown) => `sealed:${JSON.stringify(v)}`),
    identityContext: vi.fn(),
    fetchProject: vi.fn(),
    projectHeaders: vi.fn((_ctx: unknown, projectId: string) => ({
      authorization: "Bearer at_1",
      "x-project-id": projectId,
    })),
  }));

vi.mock("@/lib/api/fetch", () => ({ tryFetchWithTimeout }));
vi.mock("@/lib/api/rate-limit", () => ({
  clientKey: (_request: unknown, scope: string) => `bfrl:${scope}:203.0.113.9`,
  rateLimitRetryAfter,
}));
vi.mock("@/lib/auth/session", () => ({
  CONNECT_COOKIE: "telmoni_connect",
  cookieOpts: (maxAge: number) => ({ httpOnly: true, path: "/", maxAge }),
  getSession,
  sealConnect,
}));
vi.mock("@/lib/server/data", () => ({ identityContext, fetchProject, projectHeaders }));
vi.mock("@/lib/env", () => ({
  env: {
    AUTH_URL: "https://app.example",
    SERVER_URL: "http://notifications:8086",
  },
}));

import { NextRequest } from "next/server";

import { GET } from "./route";

const PROJECT = "project_abc";

function start(provider = "slack", query = `?project=${PROJECT}`) {
  return GET(new NextRequest(`https://app.example/connect/${provider}/start${query}`), {
    params: Promise.resolve({ provider }),
  });
}

beforeEach(() => {
  tryFetchWithTimeout.mockReset();
  sealConnect.mockClear();
  projectHeaders.mockClear();
  rateLimitRetryAfter.mockReset().mockResolvedValue(null);
  getSession.mockReset().mockResolvedValue({ userId: "usr_1" });
  identityContext
    .mockReset()
    .mockResolvedValue({ userId: "usr_1", organizationId: "org_1", accessToken: "at_1" });
  fetchProject.mockReset().mockResolvedValue({ id: PROJECT, name: "Project", role: "owner" });
  tryFetchWithTimeout.mockResolvedValue(
    new Response(
      JSON.stringify({ url: "https://slack.com/oauth/v2/authorize?state=st8", state: "st8" }),
      { status: 200 },
    ),
  );
});

describe("GET /connect/[provider]/start", () => {
  it("asks the service for the vendor URL under the project's headers, seals the handshake and redirects", async () => {
    const res = await start();
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe("https://slack.com/oauth/v2/authorize?state=st8");
    expect(res.headers.get("cache-control")).toBe("no-store, max-age=0");
    expect(tryFetchWithTimeout).toHaveBeenCalledWith(
      "http://notifications:8086/internal/connectors/slack/authorize",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({ "x-project-id": PROJECT, authorization: "Bearer at_1" }),
      }),
    );
    // The cookie carries the project and the state the callback will check.
    expect(sealConnect).toHaveBeenCalledWith({ state: "st8", projectId: PROJECT, provider: "slack" });
    expect(res.cookies.get("telmoni_connect")?.value).toBe(
      'sealed:{"state":"st8","projectId":"project_abc","provider":"slack"}',
    );
    expect(res.cookies.get("telmoni_connect")?.httpOnly).toBe(true);
  });

  it("sends a signed-out visitor through login with this URL as the return target", async () => {
    getSession.mockResolvedValue(null);
    const res = await start();
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe(
      `https://app.example/auth/login?returnTo=${encodeURIComponent(`/connect/slack/start?project=${PROJECT}`)}`,
    );
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it("answers 429 from the per-source ceiling before asking the service for a URL", async () => {
    rateLimitRetryAfter.mockResolvedValueOnce(42);
    const res = await start();
    expect(res.status).toBe(429);
    expect(res.headers.get("retry-after")).toBe("42");
    expect(rateLimitRetryAfter).toHaveBeenCalledWith("bfrl:connect:start:203.0.113.9", {
      limit: 30,
      windowMs: 60_000,
    });
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
    expect(sealConnect).not.toHaveBeenCalled();
  });

  it("does not charge a signed-out visitor's redirect against the ceiling", async () => {
    getSession.mockResolvedValue(null);
    await start();
    expect(rateLimitRetryAfter).not.toHaveBeenCalled();
  });

  // The webhook is a connector but not an OAuth one: it is connected from a
  // dialog on the page, and this route must not mint a state row for it.
  it("refuses a provider it does not know, the webhook, and a project id it cannot spell", async () => {
    let res = await start("teams");
    expect(res.headers.get("location")).toBe("https://app.example/console");
    res = await start("webhook");
    expect(res.headers.get("location")).toBe("https://app.example/console");
    res = await start("slack", "?project=../evil");
    expect(res.headers.get("location")).toBe("https://app.example/console");
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
    expect(sealConnect).not.toHaveBeenCalled();
  });

  it.each([
    [403, "forbidden"],
    [500, "authorize"],
  ])("lands a %i from the service back on the page as ?error=%s", async (status, error) => {
    tryFetchWithTimeout.mockResolvedValue(new Response("{}", { status }));
    const res = await start();
    expect(res.headers.get("location")).toBe(
      `https://app.example/${PROJECT}/connectors?error=${error}`,
    );
    expect(sealConnect).not.toHaveBeenCalled();
  });

  it("names a switched-off flag as such", async () => {
    tryFetchWithTimeout.mockResolvedValue(
      new Response(
        JSON.stringify({ type: "/errors/tenant/feature-off", flag: "connectors", status: 503 }),
        { status: 503, headers: { "content-type": "application/problem+json" } },
      ),
    );
    const res = await start();
    expect(res.headers.get("location")).toBe(`https://app.example/${PROJECT}/connectors?error=off`);
  });

  it("lands an unreachable service back on the page", async () => {
    tryFetchWithTimeout.mockResolvedValue(null);
    const res = await start();
    expect(res.headers.get("location")).toBe(
      `https://app.example/${PROJECT}/connectors?error=unavailable`,
    );
  });

  it("lands a project the caller cannot read back on the page without asking the service", async () => {
    fetchProject.mockResolvedValue(null);
    const res = await start();
    expect(res.headers.get("location")).toBe(`https://app.example/${PROJECT}/connectors?error=project`);
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it("refuses a service answer that is not a URL and a state", async () => {
    tryFetchWithTimeout.mockResolvedValue(new Response(JSON.stringify({ url: "nope" }), { status: 200 }));
    const res = await start();
    expect(res.headers.get("location")).toBe(
      `https://app.example/${PROJECT}/connectors?error=authorize`,
    );
    expect(sealConnect).not.toHaveBeenCalled();
  });
});
