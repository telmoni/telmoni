// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from "vitest";

const { tryFetchWithTimeout, getSession, sealConnect, accountHeaders, rateLimitRetryAfter } =
  vi.hoisted(() => ({
    tryFetchWithTimeout: vi.fn(),
    getSession: vi.fn(),
    rateLimitRetryAfter: vi.fn(async (): Promise<number | null> => null),
    sealConnect: vi.fn(async (v: unknown) => `sealed:${JSON.stringify(v)}`),
    accountHeaders: vi.fn(),
  }));

vi.mock("@/lib/api/fetch", async (importOriginal) => ({
  readBody: (await importOriginal<typeof import("@/lib/api/fetch")>()).readBody,
  tryFetchWithTimeout,
}));
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
vi.mock("@/lib/server/entities/identity-context", () => ({ accountHeaders }));
vi.mock("@/lib/env", () => ({
  env: {
    AUTH_URL: "https://app.example",
    SERVER_URL: "http://notifications:8086",
  },
}));

import { NextRequest } from "next/server";

import { GET } from "./route";

const ACCOUNT = "https://app.example/settings/account";
const SIGN_IN = "https://slack.com/openid/connect/authorize?scope=openid&state=st8";

function begin(provider = "slack") {
  return GET(new NextRequest(`https://app.example/connect/${provider}/identity`), {
    params: Promise.resolve({ provider }),
  });
}

beforeEach(() => {
  tryFetchWithTimeout.mockReset();
  sealConnect.mockClear();
  rateLimitRetryAfter.mockReset().mockResolvedValue(null);
  getSession.mockReset().mockResolvedValue({ userId: "usr_1" });
  accountHeaders
    .mockReset()
    .mockResolvedValue({ authorization: "Bearer at_1", "x-organization-id": "org_1" });
  tryFetchWithTimeout.mockResolvedValue(
    new Response(JSON.stringify({ url: SIGN_IN, state: "st8" }), { status: 200 }),
  );
});

describe("GET /connect/[provider]/identity", () => {
  it("asks the service for Slack's sign-in as the person, seals the state as theirs and redirects", async () => {
    const res = await begin();
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe(SIGN_IN);
    expect(res.headers.get("cache-control")).toBe("no-store, max-age=0");
    expect(tryFetchWithTimeout).toHaveBeenCalledWith(
      "http://notifications:8086/internal/v1/notifications/slack/identities/authorize",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({ authorization: "Bearer at_1" }),
      }),
    );
    // No project: the sign-in is the person's own.
    expect(sealConnect).toHaveBeenCalledWith({
      purpose: "identity",
      state: "st8",
      provider: "slack",
    });
    expect(res.cookies.get("telmoni_connect")?.httpOnly).toBe(true);
  });

  it("sends a signed-out visitor through login with this page as the return target", async () => {
    getSession.mockResolvedValue(null);
    const res = await begin();
    expect(res.headers.get("location")).toBe(
      `https://app.example/auth/login?returnTo=${encodeURIComponent("/connect/slack/identity")}`,
    );
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it("refuses a provider the console does not offer", async () => {
    const res = await begin("teams");
    expect(res.headers.get("location")).toBe("https://app.example/console");
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it("holds a flood of presses to the install's ceiling", async () => {
    rateLimitRetryAfter.mockResolvedValue(30);
    const res = await begin();
    expect(res.status).toBe(429);
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it.each([
    ["an unreachable service", null, "unavailable"],
    ["a refusal", new Response("{}", { status: 400 }), "start"],
    ["an answer of the wrong shape", new Response("{}", { status: 200 }), "start"],
  ])("lands %s back on the account's Slack section", async (_case, answer, outcome) => {
    tryFetchWithTimeout.mockResolvedValue(answer);
    const res = await begin();
    expect(res.headers.get("location")).toBe(`${ACCOUNT}?slack=${outcome}#slack`);
    expect(sealConnect).not.toHaveBeenCalled();
  });

  it("lands a person auth cannot place back on the section, asking the service nothing", async () => {
    accountHeaders.mockResolvedValue(null);
    const res = await begin();
    expect(res.headers.get("location")).toBe(`${ACCOUNT}?slack=unavailable#slack`);
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });
});
