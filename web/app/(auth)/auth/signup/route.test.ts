import { describe, it, expect, vi, beforeEach } from "vitest";

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

import { NextRequest } from "next/server";

import { rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { getAuthorizeUrl } from "@/lib/auth/oidc";
import { getSession, sealPkce } from "@/lib/auth/session";
import { GET } from "./route";

const authorizeMock = vi.mocked(getAuthorizeUrl);
const limiterMock = vi.mocked(rateLimitRetryAfter);
const sealMock = vi.mocked(sealPkce);
const getSessionMock = vi.mocked(getSession);

const req = (url: string) => new NextRequest(url);

describe("GET /auth/signup", () => {
  beforeEach(() => {
    authorizeMock.mockReset();
    authorizeMock.mockResolvedValue("https://idp.example/authorize?prompt=create");
    sealMock.mockClear();
    getSessionMock.mockReset();
    getSessionMock.mockResolvedValue(null);
    limiterMock.mockReset();
    limiterMock.mockResolvedValue(null);
  });

  it("asks for the sign-up screen, not the sign-in one", async () => {
    const res = await GET(req("http://localhost:3000/auth/signup"));
    expect(authorizeMock).toHaveBeenCalledWith(expect.any(String), { signUp: true });
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toContain("prompt=create");
    expect(res.headers.get("cache-control")).toBe("no-store, max-age=0");
  });

  it("redirects an already-authenticated visitor to returnTo directly", async () => {
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

    const res = await GET(req("http://localhost:3000/auth/signup?returnTo=%2Fsecurity"));
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe("http://localhost:3000/security");
    expect(authorizeMock).not.toHaveBeenCalled();
  });

  it("carries returnTo through the round trip, sealed", async () => {
    await GET(
      req("http://localhost:3000/auth/signup?returnTo=%2Finvites%2Faccept%3Ftoken%3Dinv_abc"),
    );
    expect(sealMock).toHaveBeenCalledWith(
      expect.objectContaining({ returnTo: "/invites/accept?token=inv_abc" }),
    );
  });

  it("refuses an off-origin returnTo", async () => {
    await GET(req("http://localhost:3000/auth/signup?returnTo=https%3A%2F%2Fevil.test%2Fx"));
    expect(sealMock).toHaveBeenCalledWith(expect.objectContaining({ returnTo: "/" }));
  });

  it("hands the address the splash collected to the hosted screen", async () => {
    await GET(req("http://localhost:3000/auth/signup?email=ada%40example.com"));
    expect(authorizeMock).toHaveBeenCalledWith(expect.any(String), {
      signUp: true,
      loginHint: "ada@example.com",
    });
  });

  it("drops a hint that is not an address rather than refusing the door", async () => {
    for (const bad of ["not-an-email", "two@@example.com", " ", "a@b"]) {
      authorizeMock.mockClear();
      const res = await GET(req(`http://localhost:3000/auth/signup?email=${encodeURIComponent(bad)}`));
      expect(res.status, bad).toBe(307);
      expect(authorizeMock, bad).toHaveBeenCalledWith(expect.any(String), { signUp: true });
    }
  });

  it("sets the pkce cookie so the callback can verify state", async () => {
    const res = await GET(req("http://localhost:3000/auth/signup"));
    expect(res.cookies.get("telmoni_pkce")?.value).toContain("sealed:");
  });

  it("answers 429 from the per-source ceiling before asking auth for an authorize URL", async () => {
    limiterMock.mockResolvedValueOnce(42);
    const res = await GET(req("http://localhost:3000/auth/signup"));
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
    const res = await GET(req("http://localhost:3000/auth/signup"));
    expect(res.status).toBe(307);
    expect(limiterMock).not.toHaveBeenCalled();
  });
});
