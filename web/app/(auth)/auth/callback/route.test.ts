// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from "vitest";

const {
  exchangeCode,
  fetchWithTimeout,
  getSession,
  sealSession,
  rateLimitRetryAfter,
} = vi.hoisted(() => ({
  exchangeCode: vi.fn(),
  fetchWithTimeout: vi.fn(),
  getSession: vi.fn(),
  sealSession: vi.fn(async () => "sealed-session"),
  rateLimitRetryAfter: vi.fn(async (): Promise<number | null> => null),
}));

vi.mock("@/lib/auth/oidc", () => ({ exchangeCode }));
vi.mock("@/lib/api/rate-limit", () => ({
  clientKey: (_request: unknown, scope: string) => `bfrl:${scope}:203.0.113.9`,
  rateLimitRetryAfter,
}));
vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout }));
vi.mock("@/lib/analytics", () => ({ track: vi.fn() }));
vi.mock("@/lib/env", () => ({
  env: {
    AUTH_URL: "https://app.example",
    SERVER_URL: "http://server:8082",
    SERVICE_SECRET: "s3cret",
  },
}));
vi.mock("@/lib/auth/session", () => ({
  SESSION_COOKIE: "telmoni_session",
  SESSION_TTL_SECONDS: 60 * 60 * 24 * 30,
  PKCE_COOKIE: "telmoni_pkce",
  cookieOpts: () => ({ httpOnly: true, path: "/" }),
  getSession,
  sealSession,
  unsealPkce: vi.fn(async () => ({ state: "st8", returnTo: "/runs" })),
}));

import { NextRequest } from "next/server";

import { unsealPkce } from "@/lib/auth/session";

import { GET } from "./route";

function callback() {
  return new NextRequest("https://app.example/auth/callback?code=c0de&state=st8", {
    headers: { cookie: "telmoni_pkce=sealed-pkce" },
  });
}

beforeEach(() => {
  fetchWithTimeout.mockReset();
  sealSession.mockClear();
  getSession.mockReset().mockResolvedValue(null);
  rateLimitRetryAfter.mockReset().mockResolvedValue(null);
  exchangeCode.mockReset().mockResolvedValue({
    userId: "user_new",
    email: "new@example.test",
    firstName: null,
    lastName: null,
    emailVerified: true,
    accessToken: "at",
    sessionId: "ses_new",
    refreshToken: "rt",
    expiresIn: 3600,
  });
});

describe("GET /auth/callback and a closed sign-up", () => {
  // ⚠ `/me` answers somebody in no organization with none, and their
  // session is sealed so they can sign in.
  it("seals the session of somebody /me places in no organization", async () => {
    fetchWithTimeout.mockResolvedValue(
      new Response(
        JSON.stringify({
          person: { userId: "user_new" },
          organizations: [],
          activeOrganizationId: null,
          sessionRowId: "sess_closed",
        }),
        { status: 200 },
      ),
    );
    const res = await GET(callback());
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe("https://app.example/runs");
    expect(res.cookies.get("telmoni_session")?.value).toBe("sealed-session");
    expect(sealSession).toHaveBeenCalledWith(
      expect.objectContaining({ sessionRowId: "sess_closed" }),
    );
  });

  it("treats any 503 from /me as the best-effort probe it always was", async () => {
    fetchWithTimeout.mockResolvedValue(
      new Response('{"type":"/errors/tenant/feature-off"}', { status: 503 }),
    );
    const res = await GET(callback());
    expect(res.headers.get("location")).toBe("https://app.example/runs");
    expect(res.cookies.get("telmoni_session")?.value).toBe("sealed-session");
    // No row was recorded, so none is sealed; the layout's own `/me` gets
    // the next chance.
    expect(sealSession).toHaveBeenCalledWith(expect.objectContaining({ sessionRowId: null }));
  });

  // The probe is a bearer lane: the token names the person and auth describes
  // them from what the exchange recorded. The body carries the browser and
  // nothing else — auth answers an email, a name or a user id with a 400,
  // and a probe that sent them sealed every session without its row id.
  it("probes /me under the bearer, sending only the browser", async () => {
    fetchWithTimeout.mockResolvedValue(new Response('{"sessionRowId":"sess_1"}', { status: 200 }));
    await GET(callback());
    const [url, init] = fetchWithTimeout.mock.calls[0] as [
      string,
      { headers: Record<string, string>; body: string },
    ];
    expect(url).toBe("http://server:8082/me");
    expect(init.headers).toEqual(
      expect.objectContaining({ authorization: "Bearer at", "x-service-secret": "s3cret" }),
    );
    expect(init.headers["x-request-id"]).toBeTruthy();
    expect(init.headers).not.toHaveProperty("x-user-id");
    expect(JSON.parse(init.body)).toEqual({ userAgent: null });
  });

  it("seals the session row /me recorded, so a sign-out elsewhere can end this browser", async () => {
    fetchWithTimeout.mockResolvedValue(new Response('{"sessionRowId":"sess_1"}', { status: 200 }));
    const res = await GET(callback());
    expect(res.cookies.get("telmoni_session")?.value).toBe("sealed-session");
    expect(sealSession).toHaveBeenCalledWith(expect.objectContaining({ sessionRowId: "sess_1" }));
  });

  // The bearer is opaque, so the session a sign-out names is the one the
  // exchange answered with, sealed beside it; nothing is read out of the token.
  it("seals the session id the exchange answered with", async () => {
    fetchWithTimeout.mockResolvedValue(new Response("{}", { status: 200 }));
    await GET(callback());
    expect(sealSession).toHaveBeenCalledWith(
      expect.objectContaining({ accessToken: "at", sessionId: "ses_new" }),
    );
  });

  it("seals the sign-in method the exchange reported", async () => {
    exchangeCode.mockResolvedValue({
      userId: "user_new",
      email: "new@example.test",
      firstName: null,
      lastName: null,
      emailVerified: true,
      accessToken: "at",
      refreshToken: "rt",
      expiresIn: 3600,
      authMethod: "google",
    });
    fetchWithTimeout.mockResolvedValue(new Response("{}", { status: 200 }));

    await GET(callback());
    expect(sealSession).toHaveBeenCalledWith(
      expect.objectContaining({ authMethod: "google" }),
    );
  });

  it("seals a null method rather than refusing a sign-in that carries none", async () => {
    fetchWithTimeout.mockResolvedValue(new Response("{}", { status: 200 }));

    const res = await GET(callback());
    expect(res.cookies.get("telmoni_session")?.value).toBe("sealed-session");
    expect(sealSession).toHaveBeenCalledWith(
      expect.objectContaining({ authMethod: null }),
    );
  });

  // Whose code this is came with the pending sign-in: the external
  // provider's when "Continue with…" sent the browser there, else auth's
  // own. The provider's id token is sealed for the sign-out that names it.
  it("spends the code at the provider the pending sign-in named, and seals its id token", async () => {
    vi.mocked(unsealPkce).mockResolvedValueOnce({
      state: "st8",
      returnTo: "/runs",
      provider: "external",
    });
    exchangeCode.mockResolvedValue({
      userId: "user_new",
      email: "new@example.test",
      firstName: null,
      lastName: null,
      emailVerified: true,
      accessToken: "at",
      refreshToken: "rt",
      expiresIn: 3600,
      idToken: "id.token.x",
      authMethod: null,
    });
    fetchWithTimeout.mockResolvedValue(new Response("{}", { status: 200 }));

    await GET(callback());
    expect(exchangeCode).toHaveBeenCalledWith("c0de", "external");
    expect(sealSession).toHaveBeenCalledWith(expect.objectContaining({ idToken: "id.token.x" }));
  });

  it("spends a code auth's own sign-in minted with no provider named, and seals no id token", async () => {
    fetchWithTimeout.mockResolvedValue(new Response("{}", { status: 200 }));

    await GET(callback());
    expect(exchangeCode).toHaveBeenCalledWith("c0de", undefined);
    expect(sealSession).toHaveBeenCalledWith(expect.objectContaining({ idToken: null }));
  });

  // ⚠ An address belongs to the person, never an organization, so that
  // collision cannot happen; the 409 `/me` can still give — a deletion racing
  // the probe — is best effort like any other miss, and the layout's own `/me`
  // answers it.
  it("seals the session as usual on a 409 from /me, rather than turning the sign-in away", async () => {
    fetchWithTimeout.mockResolvedValue(
      new Response(
        JSON.stringify({
          type: "/errors/auth/conflict",
          title: "conflict",
          status: 409,
          detail: "you do not belong to any organization right now — reload to start a new one",
        }),
        { status: 409, headers: { "content-type": "application/problem+json" } },
      ),
    );
    const res = await GET(callback());
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe("https://app.example/runs");
    expect(res.headers.get("location")).not.toContain("error=");
    expect(res.cookies.get("telmoni_session")?.value).toBe("sealed-session");
    expect(res.cookies.get("telmoni_pkce")?.value).toBe("");
    expect(sealSession).toHaveBeenCalledWith(expect.objectContaining({ sessionRowId: null }));
  });

  it("seals the session and lands on the return target when /me answers without a row", async () => {
    fetchWithTimeout.mockResolvedValue(new Response("{}", { status: 200 }));
    const res = await GET(callback());
    expect(res.headers.get("location")).toBe("https://app.example/runs");
    expect(res.cookies.get("telmoni_session")?.value).toBe("sealed-session");
    expect(sealSession).toHaveBeenCalledWith(expect.objectContaining({ sessionRowId: null }));
  });

  // ⚠ A session that expired left the organization it last stood in behind;
  // the sign-in forgets it, so `/console` opens the person's default.
  it("forgets the organization an earlier session stood in", async () => {
    fetchWithTimeout.mockResolvedValue(new Response('{"sessionRowId":"sess_1"}', { status: 200 }));
    const req = new NextRequest("https://app.example/auth/callback?code=c0de&state=st8", {
      headers: { cookie: "telmoni_pkce=sealed-pkce; telmoni-organization=org_last" },
    });
    const res = await GET(req);
    expect(res.cookies.get("telmoni_session")?.value).toBe("sealed-session");
    expect(res.cookies.get("telmoni-organization")?.value).toBe("");
  });
});

describe("GET /auth/callback duplicate and concurrent requests", () => {
  it("redirects to returnTo if exchangeCode fails but an active session already exists", async () => {
    exchangeCode.mockRejectedValue(new Error("invalid_grant"));
    getSession.mockResolvedValue({
      userId: "user_new",
      email: "new@example.test",
      firstName: null,
      lastName: null,
      accessToken: "at",
      refreshToken: "rt",
      sessionRowId: "sess_1",
      expiresAt: Date.now() + 3600000,
      idToken: null,
    });
    const res = await GET(callback());
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe("https://app.example/runs");
    expect(res.cookies.get("telmoni_pkce")?.value).toBe("");
  });

  it("redirects to /console if PKCE cookie is missing but active session exists", async () => {
    getSession.mockResolvedValue({
      userId: "user_new",
      email: "new@example.test",
      firstName: null,
      lastName: null,
      accessToken: "at",
      refreshToken: "rt",
      sessionRowId: "sess_1",
      expiresAt: Date.now() + 3600000,
      idToken: null,
    });
    const req = new NextRequest("https://app.example/auth/callback?code=c0de&state=st8");
    const res = await GET(req);
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe("https://app.example/console");
  });
});

describe("GET /auth/callback under the per-source ceiling", () => {
  it("answers 429 before spending a code exchange", async () => {
    rateLimitRetryAfter.mockResolvedValueOnce(7);
    const res = await GET(callback());
    expect(res.status).toBe(429);
    expect(res.headers.get("retry-after")).toBe("7");
    expect(rateLimitRetryAfter).toHaveBeenCalledWith("bfrl:auth:callback:203.0.113.9", {
      limit: 30,
      windowMs: 60_000,
    });
    expect(exchangeCode).not.toHaveBeenCalled();
    expect(sealSession).not.toHaveBeenCalled();
  });

  it("charges nothing for a request the cookie alone refuses", async () => {
    const req = new NextRequest("https://app.example/auth/callback?code=c0de&state=st8");
    const res = await GET(req);
    expect(res.status).toBe(307);
    expect(rateLimitRetryAfter).not.toHaveBeenCalled();
    expect(exchangeCode).not.toHaveBeenCalled();
  });
});
