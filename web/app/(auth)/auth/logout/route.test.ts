import { describe, it, expect, vi, beforeEach } from "vitest";
import { NextRequest } from "next/server";

const jar = {
  store: new Map<string, string>(),
  get(name: string) {
    const value = this.store.get(name);
    return value === undefined ? undefined : { name, value };
  },
  set(nameOrOpts: string | { name: string; value: string }, value?: string) {
    if (typeof nameOrOpts === "string") {
      this.store.set(nameOrOpts, value ?? "");
    } else {
      this.store.set(nameOrOpts.name, nameOrOpts.value);
    }
  },
  delete(name: string) {
    this.store.delete(name);
  },
};

vi.mock("next/headers", () => ({
  cookies: async () => jar,
}));

vi.mock("@/lib/auth/oidc", () => ({
  getLogoutUrl: vi.fn(),
  revokeSession: vi.fn(),
  refreshTokens: vi.fn(),
}));

import { getLogoutUrl, revokeSession } from "@/lib/auth/oidc";
import { sealSession, SESSION_COOKIE } from "@/lib/auth/session";
import { blacklistSession, clearBlacklistForTest } from "@/lib/auth/session-blacklist";
import { GET, POST } from "./route";

const mockedGetLogoutUrl = vi.mocked(getLogoutUrl);
const mockedRevoke = vi.mocked(revokeSession);

const PROVIDER_LOGOUT =
  "https://idp.example/logout" +
  "?session_id=session_42&return_to=http%3A%2F%2Flocalhost%3A3000%2F";

// The account menu's own form: a same-origin post.
const post = (site = "same-origin") =>
  new NextRequest("http://localhost:3000/auth/logout", {
    method: "POST",
    headers: { "sec-fetch-site": site },
  });

// A signed-in browser's cookie: the session the exchange answered with, and
// an opaque bearer nothing reads.
async function plantSession(sessionId: string, sessionRowId: string | null = null) {
  jar.store.set(
    SESSION_COOKIE,
    await sealSession({
      userId: "user_01",
      email: "ada@example.com",
      firstName: null,
      lastName: null,
      accessToken: "opaque_bearer",
      refreshToken: null,
      sessionId,
      expiresAt: Date.now() + 60 * 60 * 1000,
      idToken: null,
      authMethod: null,
      sessionRowId,
    }),
  );
}

beforeEach(() => {
  process.env.AUTH_SECRET = "test-secret-that-is-32-chars-long!!";
  process.env.AUTH_URL = "http://localhost:3000";
  jar.store.clear();
  clearBlacklistForTest();
  vi.clearAllMocks();
});

describe("POST /auth/logout", () => {
  // ⚠ A form on another site can post here; the browser withholds the session
  // cookie, and an unguarded answer would still clear it — a forced sign-out
  // from anywhere. The heartbeat refuses the same way.
  it("refuses a cross-site form post without touching the session", async () => {
    await plantSession("session_42");
    const sealed = jar.store.get(SESSION_COOKIE);

    const res = await POST(post("cross-site"));

    expect(res.status).toBe(303);
    expect(res.headers.get("location")).toBe("http://localhost:3000/");
    expect(jar.store.get(SESSION_COOKIE)).toBe(sealed);
    expect(mockedGetLogoutUrl).not.toHaveBeenCalled();
    expect(mockedRevoke).not.toHaveBeenCalled();
  });

  // A browser that sends no Fetch Metadata still sends `Origin` on a
  // cross-origin post; the console's own form sends its own origin.
  it("refuses a post whose Origin is another site, with no Fetch Metadata", async () => {
    await plantSession("session_42");
    const sealed = jar.store.get(SESSION_COOKIE);

    const res = await POST(
      new NextRequest("http://localhost:3000/auth/logout", {
        method: "POST",
        headers: { origin: "https://evil.example" },
      }),
    );

    expect(res.status).toBe(303);
    expect(res.headers.get("location")).toBe("http://localhost:3000/");
    expect(jar.store.get(SESSION_COOKIE)).toBe(sealed);
    expect(mockedRevoke).not.toHaveBeenCalled();

    mockedGetLogoutUrl.mockResolvedValue(null);
    const own = await POST(
      new NextRequest("http://localhost:3000/auth/logout", {
        method: "POST",
        headers: { origin: "http://localhost:3000" },
      }),
    );
    expect(own.status).toBe(303);
    expect(mockedRevoke).toHaveBeenCalledWith("session_42");
    expect(jar.store.get(SESSION_COOKIE)).toBe("");
  });

  // ⚠ The session ends at auth on EVERY sign-out. This test used to pin the
  // opposite — "skips the server revoke" — which is how a normal sign-out
  // left auth's row live and the outstanding access token accepted.
  it("ends the session at auth, then sends the browser through the provider's logout", async () => {
    await plantSession("session_42");
    mockedGetLogoutUrl.mockResolvedValue(PROVIDER_LOGOUT);

    const res = await POST(post());

    expect(res.status).toBe(303);
    expect(res.headers.get("location")).toBe(PROVIDER_LOGOUT);
    expect(mockedGetLogoutUrl).toHaveBeenCalledWith(
      "session_42",
      null,
      "http://localhost:3000/",
    );
    expect(mockedRevoke).toHaveBeenCalledWith("session_42");
    expect(jar.store.get(SESSION_COOKIE)).toBe("");
  });

  it("ends the session at auth and lands on the splash when no logout URL can be minted", async () => {
    await plantSession("session_42");
    mockedGetLogoutUrl.mockResolvedValue(null);

    const res = await POST(post());

    expect(res.status).toBe(303);
    expect(res.headers.get("location")).toBe("http://localhost:3000/");
    expect(mockedRevoke).toHaveBeenCalledWith("session_42");
    expect(jar.store.get(SESSION_COOKIE)).toBe("");
  });

  it("lands on the splash without provider calls when there is no session", async () => {
    const res = await POST(post());

    expect(res.status).toBe(303);
    expect(res.headers.get("location")).toBe("http://localhost:3000/");
    expect(mockedGetLogoutUrl).not.toHaveBeenCalled();
    expect(mockedRevoke).not.toHaveBeenCalled();
  });

  // ⚠ The provider's page is for a live session. An account deletion, a
  // confirmed email change and revoking this very device each have auth end
  // the provider session before the browser gets here, and put the row on the
  // blacklist to say so; sent through the provider's page anyway, the farewell after
  // a deletion never reached the home page. Auth is still asked to revoke at
  // the provider, so a revoke that failed at the deletion is retried here.
  it("signs out locally and lands on the splash when the session was ended already", async () => {
    await plantSession("session_42", "row_42");
    await blacklistSession("row_42");
    mockedGetLogoutUrl.mockResolvedValue(PROVIDER_LOGOUT);

    const res = await POST(post());

    expect(res.status).toBe(303);
    expect(res.headers.get("location")).toBe("http://localhost:3000/");
    expect(mockedGetLogoutUrl).not.toHaveBeenCalled();
    expect(mockedRevoke).toHaveBeenCalledWith("session_42");
    expect(jar.store.get(SESSION_COOKIE)).toBe("");
  });

  it("still goes through the provider's page for a session that was live until now", async () => {
    await plantSession("session_42", "row_42");
    mockedGetLogoutUrl.mockResolvedValue(PROVIDER_LOGOUT);

    const res = await POST(post());

    expect(res.headers.get("location")).toBe(PROVIDER_LOGOUT);
    expect(mockedRevoke).toHaveBeenCalledWith("session_42");
  });
});

describe("GET /auth/logout", () => {
  it("refuses a cross-site navigation without touching the session", async () => {
    await plantSession("session_42");
    const sealed = jar.store.get(SESSION_COOKIE);

    const res = await GET(
      new NextRequest("http://localhost:3000/auth/logout", {
        headers: { "sec-fetch-site": "cross-site" },
      }),
    );

    expect(res.headers.get("location")).toBe("http://localhost:3000/");
    expect(jar.store.get(SESSION_COOKIE)).toBe(sealed);
    expect(mockedGetLogoutUrl).not.toHaveBeenCalled();
    expect(mockedRevoke).not.toHaveBeenCalled();
  });

  it("performs the full provider sign-out on a same-origin navigation", async () => {
    await plantSession("session_42");
    mockedGetLogoutUrl.mockResolvedValue(PROVIDER_LOGOUT);

    const res = await GET(
      new NextRequest("http://localhost:3000/auth/logout", {
        headers: { "sec-fetch-site": "same-origin" },
      }),
    );

    expect(res.status).toBe(303);
    expect(res.headers.get("location")).toBe(PROVIDER_LOGOUT);
  });
});
