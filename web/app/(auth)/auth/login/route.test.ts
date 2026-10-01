import { describe, it, expect, vi, beforeEach } from "vitest";
import { NextRequest } from "next/server";

vi.mock("@/lib/auth/oidc", () => ({ getAuthorizeUrl: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  clientKey: (_request: unknown, scope: string) => `bfrl:${scope}:203.0.113.9`,
  rateLimitRetryAfter: vi.fn(async () => null),
}));
vi.mock("@/lib/auth/session", () => ({
  PKCE_COOKIE: "telmoni_pkce",
  cookieOpts: () => ({ httpOnly: true, path: "/" }),
  getSession: vi.fn(async () => null),
  sealPkce: vi.fn(async (v: unknown) => `sealed:${JSON.stringify(v)}`),
}));

import { rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { getAuthorizeUrl } from "@/lib/auth/oidc";
import { getSession, sealPkce } from "@/lib/auth/session";
import { GET } from "./route";

const authorizeMock = vi.mocked(getAuthorizeUrl);
const limiterMock = vi.mocked(rateLimitRetryAfter);
const sealMock = vi.mocked(sealPkce);
const getSessionMock = vi.mocked(getSession);

const req = (url: string) => new NextRequest(url);

describe("GET /auth/login", () => {
  beforeEach(() => {
    authorizeMock.mockReset();
    authorizeMock.mockResolvedValue("https://idp.example/authorize");
    sealMock.mockClear();
    getSessionMock.mockReset();
    getSessionMock.mockResolvedValue(null);
    limiterMock.mockReset();
    limiterMock.mockResolvedValue(null);
  });

  it("redirects an unauthenticated visitor to the provider's authorize URL with CSRF state", async () => {
    const res = await GET(req("http://localhost:3000/auth/login"));
    expect(authorizeMock).toHaveBeenCalledWith(expect.any(String), {});
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe("https://idp.example/authorize");
    expect(res.headers.get("cache-control")).toBe("no-store, max-age=0");
    expect(res.cookies.get("telmoni_pkce")?.value).toContain("sealed:");
  });

  it("redirects an already-authenticated visitor to returnTo directly without hitting the provider", async () => {
    getSessionMock.mockResolvedValue({
      userId: "user_123",
      email: "ada@example.com",
      firstName: "Ada",
      lastName: "Lovelace",
      accessToken: "token",
      refreshToken: "refresh",
      sessionId: "ses_123",
      sessionRowId: "row_123",
      expiresAt: Date.now() + 60_000,
      idToken: null,
      authMethod: null,
    });

    const res = await GET(req("http://localhost:3000/auth/login?returnTo=%2Fsecurity"));
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe("http://localhost:3000/security");
    expect(authorizeMock).not.toHaveBeenCalled();
  });

  it("defaults returnTo to /console when none provided for authenticated visitor", async () => {
    getSessionMock.mockResolvedValue({
      userId: "user_123",
      email: "ada@example.com",
      firstName: "Ada",
      lastName: "Lovelace",
      accessToken: "token",
      refreshToken: "refresh",
      sessionId: "ses_123",
      sessionRowId: "row_123",
      expiresAt: Date.now() + 60_000,
      idToken: null,
      authMethod: null,
    });

    const res = await GET(req("http://localhost:3000/auth/login"));
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe("http://localhost:3000/console");
    expect(authorizeMock).not.toHaveBeenCalled();
  });

  it("carries returnTo through PKCE sealing for unauthenticated visitor", async () => {
    await GET(
      req("http://localhost:3000/auth/login?returnTo=%2Fproject_abc123%2Fsettings"),
    );
    expect(sealMock).toHaveBeenCalledWith(
      expect.objectContaining({ returnTo: "/project_abc123/settings" }),
    );
  });

  it("refuses an off-origin returnTo", async () => {
    await GET(req("http://localhost:3000/auth/login?returnTo=https%3A%2F%2Fevil.test%2Fx"));
    expect(sealMock).toHaveBeenCalledWith(expect.objectContaining({ returnTo: "/" }));
  });

  it("returns 503 when the auth provider fails to mint authorize URL", async () => {
    authorizeMock.mockRejectedValue(new Error("auth service down"));
    const res = await GET(req("http://localhost:3000/auth/login"));
    expect(res.status).toBe(503);
    expect(res.headers.get("retry-after")).toBe("60");
  });

  it("answers 429 from the per-source ceiling before asking auth for an authorize URL", async () => {
    limiterMock.mockResolvedValueOnce(42);
    const res = await GET(req("http://localhost:3000/auth/login"));
    expect(res.status).toBe(429);
    expect(res.headers.get("retry-after")).toBe("42");
    expect(res.headers.get("cache-control")).toBe("no-store, max-age=0");
    expect(limiterMock).toHaveBeenCalledWith("bfrl:auth:start:203.0.113.9", {
      limit: 60,
      windowMs: 60_000,
    });
    expect(authorizeMock).not.toHaveBeenCalled();
    expect(res.cookies.get("telmoni_pkce")).toBeUndefined();
  });

  it("does not charge a signed-in visitor's redirect against the ceiling", async () => {
    getSessionMock.mockResolvedValue({
      userId: "user_123",
      email: "ada@example.com",
      firstName: "Ada",
      lastName: "Lovelace",
      accessToken: "token",
      refreshToken: "refresh",
      sessionId: "ses_123",
      sessionRowId: "row_123",
      expiresAt: Date.now() + 60_000,
      idToken: null,
      authMethod: null,
    });
    const res = await GET(req("http://localhost:3000/auth/login"));
    expect(res.status).toBe(307);
    expect(limiterMock).not.toHaveBeenCalled();
  });
});
