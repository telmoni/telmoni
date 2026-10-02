// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from "vitest";

const {
  tryFetchWithTimeout,
  getSession,
  sealConnect,
  identityContext,
  fetchProjectAnywhere,
  projectHeaders,
  rateLimitRetryAfter,
} =
  vi.hoisted(() => ({
    tryFetchWithTimeout: vi.fn(),
    getSession: vi.fn(),
    rateLimitRetryAfter: vi.fn(async (): Promise<number | null> => null),
    sealConnect: vi.fn(async (v: unknown) => `sealed:${JSON.stringify(v)}`),
    identityContext: vi.fn(),
    fetchProjectAnywhere: vi.fn(),
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
vi.mock("@/lib/server/data", () => ({ identityContext, fetchProjectAnywhere, projectHeaders }));
vi.mock("@/lib/env", () => ({
  env: {
    AUTH_URL: "https://app.example",
    SERVER_URL: "http://notifications:8086",
  },
}));

import { NextRequest } from "next/server";

import { GET } from "./route";

const PROJECT = "project_abc";
// The project's Connectors page, by the slugs its path is spelled with.
const CONNECTORS = "https://app.example/acme/platform/connectors";

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
  // The request stands in `org_elsewhere` — whichever organization a tab
  // opened last — while the project is `org_home`'s.
  identityContext
    .mockReset()
    .mockResolvedValue({ userId: "usr_1", organizationId: "org_elsewhere", accessToken: "at_1" });
  fetchProjectAnywhere.mockReset().mockResolvedValue({
    id: PROJECT,
    slug: "platform",
    name: "Platform",
    role: "owner",
    organizationId: "org_home",
    organizationSlug: "acme",
  });
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
    // The cookie carries the project, its organization and the state the
    // callback will check: ids, which a rename in the meantime does not move.
    expect(sealConnect).toHaveBeenCalledWith({
      state: "st8",
      organizationId: "org_home",
      projectId: PROJECT,
      provider: "slack",
    });
    expect(res.cookies.get("telmoni_connect")?.value).toBe(
      'sealed:{"state":"st8","organizationId":"org_home","projectId":"project_abc","provider":"slack"}',
    );
    expect(res.cookies.get("telmoni_connect")?.httpOnly).toBe(true);
  });

  // ⚠ This path names no organization, so the request stands wherever the
  // cookie last pointed. The project's own organization is the one to name.
  it("acts in the project's organization, not the one the request stands in", async () => {
    await start();
    expect(fetchProjectAnywhere).toHaveBeenCalledWith(PROJECT);
    expect(projectHeaders).toHaveBeenCalledWith(
      expect.objectContaining({ organizationId: "org_home", accessToken: "at_1" }),
      PROJECT,
    );
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
    expect(res.headers.get("location")).toBe(`${CONNECTORS}?error=${error}`);
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
    expect(res.headers.get("location")).toBe(`${CONNECTORS}?error=off`);
  });

  it("lands an unreachable service back on the page", async () => {
    tryFetchWithTimeout.mockResolvedValue(null);
    const res = await start();
    expect(res.headers.get("location")).toBe(`${CONNECTORS}?error=unavailable`);
  });

  // There is no page to land back on: its path is spelled with slugs only a
  // project the caller can read gives.
  it("sends a project the caller cannot read to the console's door without asking the service", async () => {
    fetchProjectAnywhere.mockResolvedValue(null);
    const res = await start();
    expect(res.headers.get("location")).toBe("https://app.example/console");
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
    expect(sealConnect).not.toHaveBeenCalled();
  });

  it("refuses a service answer that is not a URL and a state", async () => {
    tryFetchWithTimeout.mockResolvedValue(new Response(JSON.stringify({ url: "nope" }), { status: 200 }));
    const res = await start();
    expect(res.headers.get("location")).toBe(`${CONNECTORS}?error=authorize`);
    expect(sealConnect).not.toHaveBeenCalled();
  });
});
