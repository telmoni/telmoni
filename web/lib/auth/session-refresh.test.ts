import { describe, it, expect, vi, beforeEach } from "vitest";

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
  refreshSessionTokens,
  sealSession,
  SESSION_COOKIE,
  type SessionData,
} from "./session";

const mockedRefresh = vi.mocked(refreshTokens);

function granted(overrides: Record<string, unknown> = {}) {
  return {
    kind: "granted" as const,
    tokens: {
      userId: "user_01",
      email: "ada@example.com",
      emailVerified: true,
      firstName: "Ada",
      lastName: null,
      accessToken: "at_new",
      sessionId: "ses_1",
      refreshToken: "rt_new" as string | null,
      expiresIn: 3600,
      ...overrides,
    },
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

// ⚠ A page cannot write the cookie, and auth ends the whole session when a
// refresh token comes back past its grace: a refresh made while a page
// rendered was lost with it, and the next render's spent the same token. The
// proxy renews a lapsing session before anything renders
// (`lib/proxy/session.ts`); reading one never spends its token.
describe("getSession reads the session, and never renews it", () => {
  it("returns the session untouched while the token is fresh", async () => {
    await seed(session());

    const result = await getSession();

    expect(result?.accessToken).toBe("at_old");
    expect(mockedRefresh).not.toHaveBeenCalled();
  });

  it("returns a session about to lapse as it is, without spending its refresh token", async () => {
    await seed(session({ expiresAt: Date.now() + 30_000 }));

    const result = await getSession();

    expect(result?.accessToken).toBe("at_old");
    expect(result?.refreshToken).toBe("rt_old");
    expect(mockedRefresh).not.toHaveBeenCalled();
    expect(jar.deleted).toEqual([]);
  });

  it("returns a lapsed session, and one sealed with no expiry, as they are", async () => {
    await seed(session({ expiresAt: Date.now() - 1_000 }));
    expect((await getSession())?.accessToken).toBe("at_old");

    await seed(session({ expiresAt: 0 }));
    expect((await getSession())?.accessToken).toBe("at_old");

    expect(mockedRefresh).not.toHaveBeenCalled();
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

  it("answers nobody for a cookie that does not unseal", async () => {
    jar.store.set(SESSION_COOKIE, "not-a-sealed-session");

    expect(await getSession()).toBeNull();
  });
});

describe("refreshSessionTokens", () => {
  // Auth marks the row seen as it rotates the tokens, so the row has to ride
  // the refresh — there is no separate touch for it to ride any more.
  it("renews with the rotated pair, naming the session row, and keeps it", async () => {
    mockedRefresh.mockResolvedValue(granted());

    const renewal = await refreshSessionTokens(
      session({ expiresAt: Date.now() + 30_000, sessionRowId: "sess_1" }),
    );

    expect(mockedRefresh).toHaveBeenCalledWith("rt_old", "sess_1");
    expect(renewal).toMatchObject({
      kind: "renewed",
      session: { accessToken: "at_new", refreshToken: "rt_new", sessionId: "ses_1", sessionRowId: "sess_1" },
    });
    if (renewal.kind !== "renewed") throw new Error("not renewed");
    expect(renewal.session.expiresAt).toBeGreaterThan(Date.now() + 3500 * 1000);
  });

  it("carries the sign-in method across a renewal", async () => {
    mockedRefresh.mockResolvedValue(granted());

    const renewal = await refreshSessionTokens(session({ authMethod: "google" }));

    expect(renewal).toMatchObject({ kind: "renewed", session: { authMethod: "google" } });
  });

  it("keeps the old refresh token when the provider doesn't rotate it", async () => {
    mockedRefresh.mockResolvedValue(granted({ refreshToken: null }));

    const renewal = await refreshSessionTokens(session());

    expect(renewal).toMatchObject({ kind: "renewed", session: { refreshToken: "rt_old" } });
  });

  it("refuses a session with no refresh token, without asking auth", async () => {
    expect(await refreshSessionTokens(session({ refreshToken: null }))).toEqual({
      kind: "refused",
    });
    expect(mockedRefresh).not.toHaveBeenCalled();
  });

  it("passes auth's refusal and its silence through, as different answers", async () => {
    mockedRefresh.mockResolvedValueOnce({ kind: "refused" });
    expect(await refreshSessionTokens(session())).toEqual({ kind: "refused" });

    mockedRefresh.mockResolvedValueOnce({ kind: "unreachable" });
    expect(await refreshSessionTokens(session())).toEqual({ kind: "unreachable" });
  });
});
