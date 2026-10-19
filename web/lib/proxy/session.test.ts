import { beforeEach, describe, expect, it, vi } from "vitest";
import { unsealData } from "iron-session";

vi.mock("@/lib/auth/oidc", () => ({
  refreshTokens: vi.fn(),
}));

import { NextRequest, NextResponse } from "next/server";

import { refreshTokens, type RefreshAnswer } from "@/lib/auth/oidc";
import { blacklistSession, clearBlacklistForTest } from "@/lib/auth/session-blacklist";
import { sealSession, type SessionData } from "@/lib/auth/session";
import { logger } from "@/lib/logger";
import { proxy } from "@/proxy";

import { forgetRenewalsForTest, settleSession } from "./session";

const SECRET = "test-secret-that-is-32-chars-long!!";
const mockedRefresh = vi.mocked(refreshTokens);

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
    sessionRowId: null,
    expiresAt: Date.now() + 60 * 60 * 1000,
    idToken: null,
    ...overrides,
  };
}

function granted(): RefreshAnswer {
  return {
    kind: "granted",
    tokens: {
      userId: "user_01",
      email: "ada@example.com",
      emailVerified: true,
      firstName: "Ada",
      lastName: null,
      accessToken: "at_new",
      sessionId: "ses_1",
      refreshToken: "rt_new",
      expiresIn: 900,
    },
  };
}

async function cookie(overrides: Partial<SessionData> = {}): Promise<string> {
  return sealSession(session(overrides));
}

beforeEach(() => {
  process.env.AUTH_SECRET = SECRET;
  mockedRefresh.mockReset();
  clearBlacklistForTest();
  forgetRenewalsForTest();
});

describe("settleSession", () => {
  it("finds nobody without a cookie, or with one that does not unseal", async () => {
    expect(await settleSession(undefined)).toEqual({ kind: "none" });
    expect(await settleSession("not-a-sealed-session")).toEqual({ kind: "none" });
  });

  it("keeps a session whose bearer has time left, and asks auth nothing", async () => {
    expect(await settleSession(await cookie())).toEqual({ kind: "kept" });
    expect(mockedRefresh).not.toHaveBeenCalled();
  });

  // The renewed cookie is what the request renders with and the browser
  // keeps: the rotated pair, and everything else the session carried.
  it("renews a session about to lapse, into a cookie holding the rotated pair", async () => {
    mockedRefresh.mockResolvedValue(granted());

    const settled = await settleSession(
      await cookie({ expiresAt: Date.now() + 30_000, sessionRowId: "sess_1", authMethod: "google" }),
    );

    expect(mockedRefresh).toHaveBeenCalledWith("rt_old", "sess_1");
    if (settled.kind !== "renewed") throw new Error(`settled as ${settled.kind}`);
    const renewed = await unsealData<SessionData>(settled.sealed, { password: SECRET });
    expect(renewed).toMatchObject({
      accessToken: "at_new",
      refreshToken: "rt_new",
      sessionRowId: "sess_1",
      authMethod: "google",
    });
    expect(renewed.expiresAt).toBeGreaterThan(Date.now() + 800 * 1000);
  });

  it("renews a lapsed session, and one sealed with no expiry", async () => {
    mockedRefresh.mockResolvedValue(granted());

    expect((await settleSession(await cookie({ expiresAt: Date.now() - 1_000, refreshToken: "rt_a" }))).kind).toBe(
      "renewed",
    );
    expect((await settleSession(await cookie({ expiresAt: 0, refreshToken: "rt_b" }))).kind).toBe("renewed");
  });

  it("ends a session auth refuses to renew, and one with nothing to renew it with", async () => {
    mockedRefresh.mockResolvedValue({ kind: "refused" });
    expect(await settleSession(await cookie({ expiresAt: Date.now() - 1_000 }))).toEqual({ kind: "ended" });

    mockedRefresh.mockClear();
    expect(
      await settleSession(await cookie({ expiresAt: Date.now() - 1_000, refreshToken: null })),
    ).toEqual({ kind: "ended" });
    expect(mockedRefresh).not.toHaveBeenCalled();
  });

  // Auth down is no verdict on the session: signing everybody out while it is
  // would turn an outage of a minute into a sign-in for every reader.
  it("keeps a session while auth does not answer, for the next request to renew", async () => {
    mockedRefresh.mockResolvedValue({ kind: "unreachable" });

    expect(
      await settleSession(await cookie({ expiresAt: Date.now() - 1_000, refreshToken: "rt_down" })),
    ).toEqual({ kind: "kept" });
  });

  // The pages sign an ended session out themselves, and say how
  // (`sessionEndedAlready`): to the home page after an account's deletion.
  it("leaves a session the console ended to the pages, and spends nothing on it", async () => {
    await blacklistSession("sess_ended");

    expect(
      await settleSession(await cookie({ expiresAt: Date.now() - 1_000, sessionRowId: "sess_ended" })),
    ).toEqual({ kind: "kept" });
    expect(mockedRefresh).not.toHaveBeenCalled();
  });

  // A page's requests arrive together — the document, its prefetches, the
  // event stream — each with the same cookie.
  it("spends a refresh token once for the requests that arrive with it together", async () => {
    let answer!: (value: RefreshAnswer) => void;
    mockedRefresh.mockReturnValue(new Promise((resolve) => (answer = resolve)));
    const sent = await cookie({ expiresAt: Date.now() + 30_000, refreshToken: "rt_shared" });

    const settling = [settleSession(sent), settleSession(sent), settleSession(sent)];
    await vi.waitFor(() => expect(mockedRefresh).toHaveBeenCalled());
    answer(granted());
    const [first, second, third] = await Promise.all(settling);

    expect(mockedRefresh).toHaveBeenCalledTimes(1);
    expect(first.kind).toBe("renewed");
    expect(second).toEqual(first);
    expect(third).toEqual(first);
  });

  // A prefetch the page sent before the renewed cookie reached the browser
  // still carries the old one.
  it("answers a request that left with the old cookie with the renewal already made", async () => {
    mockedRefresh.mockResolvedValue(granted());
    const sent = await cookie({ expiresAt: Date.now() + 30_000, refreshToken: "rt_late" });

    const first = await settleSession(sent);
    const late = await settleSession(sent);

    expect(mockedRefresh).toHaveBeenCalledTimes(1);
    expect(late).toEqual(first);
  });

  // Auth may have spent the token before its answer was lost: past its grace,
  // asking again would read as a stolen copy, so no answer is kept only a
  // moment, and an answer long enough for the page's stragglers.
  it("asks again a moment after auth did not answer, and reuses an answer for longer", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout"] });
    try {
      mockedRefresh.mockResolvedValue({ kind: "unreachable" });
      const silent = await cookie({ expiresAt: Date.now() + 30_000, refreshToken: "rt_silent" });
      expect(await settleSession(silent)).toEqual({ kind: "kept" });
      await vi.advanceTimersByTimeAsync(2_000);
      await settleSession(silent);
      expect(mockedRefresh).toHaveBeenCalledTimes(2);

      mockedRefresh.mockReset();
      mockedRefresh.mockResolvedValue(granted());
      const answered = await cookie({ expiresAt: Date.now() + 30_000, refreshToken: "rt_answered" });
      await settleSession(answered);
      await vi.advanceTimersByTimeAsync(2_000);
      await settleSession(answered);
      expect(mockedRefresh).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  // Every request the console answers passes through here.
  it("keeps the cookie as it came when a renewal throws, and names no token in its line", async () => {
    const error = vi.spyOn(logger, "error").mockImplementation(() => {});
    mockedRefresh.mockRejectedValue(new Error("pool closed"));

    expect(
      await settleSession(await cookie({ expiresAt: Date.now() - 1_000, refreshToken: "rt_throws" })),
    ).toEqual({ kind: "kept" });
    expect(error).toHaveBeenCalledOnce();
    expect(JSON.stringify(error.mock.calls)).not.toContain("rt_throws");
    error.mockRestore();
  });
});

// The proxy is the one place a renewal is written, so what it writes, and
// where, is held here: into the request every page, action and handler below
// reads, and onto the answer the browser keeps.
describe("the proxy's session cookie", () => {
  async function send(path: string, overrides: Partial<SessionData>): Promise<NextResponse> {
    const value = await cookie(overrides);
    const res = await proxy(
      new NextRequest(`http://localhost:3000${path}`, {
        headers: { cookie: `telmoni_session=${value}` },
      }),
      {} as never,
    );
    if (!(res instanceof NextResponse)) throw new Error("the proxy answered no NextResponse");
    return res;
  }

  it("writes a renewed session into the request the page reads and onto the answer", async () => {
    mockedRefresh.mockResolvedValue(granted());

    const res = await send("/acme/web/traces", { expiresAt: Date.now() + 30_000 });

    const sealed = res.cookies.get("telmoni_session")?.value ?? "";
    const renewed = await unsealData<SessionData>(sealed, { password: SECRET });
    expect(renewed.accessToken).toBe("at_new");
    expect(res.headers.get("x-middleware-request-cookie")).toContain(`telmoni_session=${sealed}`);
  });

  it("clears an ended session, sending a page to sign in and refusing a read", async () => {
    mockedRefresh.mockResolvedValue({ kind: "refused" });

    const page = await send("/acme/web/traces", { expiresAt: Date.now() - 1_000, refreshToken: "rt_a" });
    expect(page.status).toBe(307);
    expect(page.headers.get("location")).toContain("/auth/login?returnTo=%2Facme%2Fweb%2Ftraces");
    expect(page.cookies.get("telmoni_session")?.value).toBe("");

    const read = await send("/api/v1/events", { expiresAt: Date.now() - 1_000, refreshToken: "rt_b" });
    expect(read.status).toBe(401);
    expect(read.cookies.get("telmoni_session")?.value).toBe("");
  });

  // The callback writes a new session and the sign-out clears it: a renewal
  // there would only spend a token on a session they replace.
  it.each(["/auth/callback", "/auth/logout"])("leaves %s's cookie to it", async (path) => {
    const res = await send(path, { expiresAt: Date.now() - 1_000 });

    expect(mockedRefresh).not.toHaveBeenCalled();
    expect(res.cookies.get("telmoni_session")).toBeUndefined();
  });

  // The device approval's action spends the bearer, and the auth pages render
  // no heartbeat to renew it.
  it("renews on an auth page that reads the session, the device approval", async () => {
    mockedRefresh.mockResolvedValue(granted());

    const res = await send("/auth/device", { expiresAt: Date.now() - 1_000, refreshToken: "rt_device" });

    expect(mockedRefresh).toHaveBeenCalledWith("rt_device", null);
    const sealed = res.cookies.get("telmoni_session")?.value ?? "";
    expect((await unsealData<SessionData>(sealed, { password: SECRET })).accessToken).toBe("at_new");
  });
});
