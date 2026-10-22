import { beforeEach, describe, expect, it, vi } from "vitest";
import { NextRequest } from "next/server";

vi.mock("@/lib/auth/oidc", () => ({ getAuthorizeUrl: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  clientKey: (_request: unknown, scope: string) => `bfrl:${scope}:203.0.113.9`,
  rateLimitRetryAfter: vi.fn(async () => null),
}));
vi.mock("@/lib/auth/session", () => ({
  PKCE_COOKIE: "telmoni_pkce",
  cookieOpts: () => ({ httpOnly: true, path: "/" }),
  sealPkce: vi.fn(async (v: unknown) => `sealed:${JSON.stringify(v)}`),
  unsealPkce: vi.fn(async (sealed: string) =>
    sealed.startsWith("sealed:") ? JSON.parse(sealed.slice("sealed:".length)) : null,
  ),
}));
vi.mock("@/lib/env", () => ({ env: { AUTH_URL: "http://localhost:3000" } }));

import { rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { getAuthorizeUrl } from "@/lib/auth/oidc";
import { challengeOf } from "@/lib/auth/pkce";
import { GET } from "./route";

const authorizeMock = vi.mocked(getAuthorizeUrl);
const limiterMock = vi.mocked(rateLimitRetryAfter);

function visit(pkce?: object) {
  return new NextRequest("http://localhost:3000/auth/login/external", {
    headers: pkce ? { cookie: `telmoni_pkce=sealed:${JSON.stringify(pkce)}` } : {},
  });
}

describe("GET /auth/login/external", () => {
  beforeEach(() => {
    authorizeMock.mockReset();
    authorizeMock.mockResolvedValue("https://idp.example/authorize?state=st8");
    limiterMock.mockReset();
    limiterMock.mockResolvedValue(null);
  });

  it("carries the pending sign-in on to the provider, marked as the provider's", async () => {
    const res = await GET(visit({ state: "st8", returnTo: "/runs" }));
    expect(authorizeMock).toHaveBeenCalledWith("st8", {
      codeChallenge: expect.stringMatching(/^[A-Za-z0-9_-]{43}$/),
      nonce: expect.stringMatching(/^[A-Za-z0-9_-]{43}$/),
      provider: "external",
    });
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe("https://idp.example/authorize?state=st8");
    expect(res.headers.get("cache-control")).toBe("no-store, max-age=0");
    // A fresh proof for the provider, its challenge the sealed verifier's.
    const sealed = JSON.parse(res.cookies.get("telmoni_pkce")!.value.replace(/^sealed:/, ""));
    expect(sealed).toMatchObject({ state: "st8", returnTo: "/runs", provider: "external" });
    const [, sent] = authorizeMock.mock.calls[0]!;
    expect(sent.codeChallenge).toBe(await challengeOf(sealed.verifier));
    expect(sent.nonce).toBe(sealed.nonce);
  });

  // ⚠ Without the door's state there is nothing for the callback to check
  // against, so this never starts a sign-in of its own.
  it("sends a visit with no sign-in in progress back through the door", async () => {
    const res = await GET(visit());
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe("http://localhost:3000/auth/login");
    expect(authorizeMock).not.toHaveBeenCalled();
  });

  it("answers a 503 when auth cannot start the sign-in", async () => {
    authorizeMock.mockRejectedValue(new Error("auth start failed: 503"));
    const res = await GET(visit({ state: "st8", returnTo: "/console" }));
    expect(res.status).toBe(503);
  });

  it("shares the doors' ceiling", async () => {
    limiterMock.mockResolvedValue(30);
    const res = await GET(visit({ state: "st8", returnTo: "/console" }));
    expect(res.status).toBe(429);
    expect(res.headers.get("retry-after")).toBe("30");
    expect(authorizeMock).not.toHaveBeenCalled();
  });
});
