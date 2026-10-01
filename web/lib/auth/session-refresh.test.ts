import { describe, it, expect, vi, beforeEach } from "vitest";
import { unsealData } from "iron-session";

const jar = {
  store: new Map<string, string>(),
  deleted: [] as string[],
  get(name: string) {
    const value = this.store.get(name);
    return value === undefined ? undefined : { name, value };
  },
  set(opts: { name: string; value: string }) {
    this.store.set(opts.name, opts.value);
  },
  delete(name: string) {
    this.deleted.push(name);
    this.store.delete(name);
  },
};

vi.mock("next/headers", () => ({
  cookies: async () => jar,
}));

vi.mock("@/lib/auth/oidc", () => ({
  refreshTokens: vi.fn(),
}));

import { refreshTokens } from "@/lib/auth/oidc";
import {
  blacklistSession,
  clearBlacklistForTest,
} from "./session-blacklist";
import {
  getSession,
  sealSession,
  SESSION_COOKIE,
  type SessionData,
} from "./session";

const mockedRefresh = vi.mocked(refreshTokens);

function authnResult(
  overrides: Partial<Awaited<ReturnType<typeof refreshTokens>>> = {},
) {
  return {
    userId: "user_01",
    email: "ada@example.com",
    emailVerified: true,
    firstName: "Ada",
    lastName: null,
    accessToken: "at_new",
    sessionId: "ses_1",
    refreshToken: "rt_new",
    expiresIn: 3600,
    ...overrides,
  };
}

function session(overrides: Partial<SessionData> = {}): SessionData {
  return {
    userId: "user_01",
    email: "ada@example.com",
    firstName: "Ada",
    lastName: null,
    authMethod: null,
    accessToken: "at_old",
    refreshToken: "rt_old",
    sessionId: "ses_1",
    expiresAt: Date.now() + 60 * 60 * 1000,
    idToken: null,
    sessionRowId: null,
    ...overrides,
  };
}

beforeEach(async () => {
  process.env.AUTH_SECRET = "test-secret-that-is-32-chars-long!!";
  jar.store.clear();
  jar.deleted = [];
  mockedRefresh.mockReset();
  clearBlacklistForTest();
});

async function seed(data: SessionData) {
  jar.store.set(SESSION_COOKIE, await sealSession(data));
}

describe("getSession refresh-on-use", () => {
  it("returns the session untouched while the token is fresh", async () => {
    await seed(session());

    const result = await getSession();

    expect(result?.accessToken).toBe("at_old");
    expect(mockedRefresh).not.toHaveBeenCalled();
  });

  it("refreshes + re-seals when the token is within the 60s threshold", async () => {
    await seed(session({ expiresAt: Date.now() + 30_000 }));
    mockedRefresh.mockResolvedValue(
      authnResult({ accessToken: "at_new", refreshToken: "rt_new" }),
    );

    const result = await getSession();

    expect(mockedRefresh).toHaveBeenCalledWith("rt_old", null);
    expect(result?.accessToken).toBe("at_new");
    expect(result?.refreshToken).toBe("rt_new");
    expect(result!.expiresAt).toBeGreaterThan(Date.now() + 3500 * 1000);

    const resealed = jar.store.get(SESSION_COOKIE)!;
    const recovered = await unsealData<SessionData>(resealed, {
      password: process.env.AUTH_SECRET!,
    });
    expect(recovered.accessToken).toBe("at_new");
  });

  it("carries the sign-in method across a token refresh", async () => {
    await seed(session({ expiresAt: Date.now() + 30_000, authMethod: "google" }));
    mockedRefresh.mockResolvedValue(authnResult());

    const result = await getSession();

    expect(result?.authMethod).toBe("google");
  });

  // Auth marks the row seen as it rotates the tokens, so the row has to ride
  // the refresh — there is no separate touch for it to ride any more.
  it("names the session row on the refresh, and keeps it", async () => {
    await seed(session({ expiresAt: Date.now() + 30_000, sessionRowId: "sess_1" }));
    mockedRefresh.mockResolvedValue(authnResult());

    const result = await getSession();

    expect(mockedRefresh).toHaveBeenCalledWith("rt_old", "sess_1");
    expect(result?.sessionRowId).toBe("sess_1");
    expect(result?.sessionId).toBe("ses_1");
  });

  it("keeps the old refresh token when the provider doesn't rotate it", async () => {
    await seed(session({ expiresAt: Date.now() + 30_000 }));
    mockedRefresh.mockResolvedValue(
      authnResult({ accessToken: "at_new", refreshToken: null }),
    );

    const result = await getSession();

    expect(result?.refreshToken).toBe("rt_old");
  });

  it("clears the cookie and returns null when the refresh grant fails", async () => {
    await seed(session({ expiresAt: Date.now() + 30_000 }));
    mockedRefresh.mockResolvedValue(null);

    const result = await getSession();

    expect(result).toBeNull();
    expect(jar.deleted).toContain(SESSION_COOKIE);
  });

  it("clears the cookie when an expired session has no refresh token", async () => {
    await seed(session({ expiresAt: Date.now() + 30_000, refreshToken: null }));

    const result = await getSession();

    expect(result).toBeNull();
    expect(mockedRefresh).not.toHaveBeenCalled();
    expect(jar.deleted).toContain(SESSION_COOKIE);
  });

  // Every session is a real one now, the test door's included, so none is
  // sealed without an expiry: a cookie that carries none is refreshed like an
  // expired one rather than taken as immortal.
  it("refreshes a session sealed without an expiry instead of keeping it forever", async () => {
    await seed(session({ expiresAt: 0 }));
    mockedRefresh.mockResolvedValue(authnResult());

    const result = await getSession();

    expect(mockedRefresh).toHaveBeenCalledWith("rt_old", null);
    expect(result?.accessToken).toBe("at_new");
  });

  it("kills session immediately when sessionRowId is blacklisted, even if token is fresh", async () => {
    const s = session({ sessionRowId: "sess_revoked_123" });
    await seed(s);
    await blacklistSession("sess_revoked_123");

    const result = await getSession();

    expect(result).toBeNull();
    expect(jar.deleted).toContain(SESSION_COOKIE);
    expect(mockedRefresh).not.toHaveBeenCalled();
  });

  it("flags needsReseal=true when re-sealing throws in read-only Server Component context", async () => {
    await seed(session({ expiresAt: Date.now() + 30_000 }));
    mockedRefresh.mockResolvedValue(
      authnResult({ accessToken: "at_new", refreshToken: "rt_new" }),
    );

    const originalSet = jar.set;
    jar.set = () => {
      throw new Error("ReadonlyRequestCookies cannot be modified");
    };

    try {
      const result = await getSession();
      expect(result?.accessToken).toBe("at_new");
      expect(result?.needsReseal).toBe(true);
    } finally {
      jar.set = originalSet;
    }
  });
});
