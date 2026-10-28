import { beforeEach, describe, expect, it, vi } from "vitest";

const mockGetSession = vi.fn();
const mockRefreshSessionTokens = vi.fn();
const mockSealSession = vi.fn();
const mockSessionEndedAlready = vi.fn();

vi.mock("@/lib/auth/session", () => ({
  SESSION_COOKIE: "telmoni_session",
  // Restated, like SESSION_COOKIE above: a factory mock replaces the whole
  // module, so an export the route reads has to exist here or the import
  // throws before a single assertion runs.
  SESSION_TTL_SECONDS: 60 * 60 * 24 * 30,
  cookieOpts: vi.fn(() => ({ httpOnly: true, path: "/" })),
  getSession: () => mockGetSession(),
  refreshSessionTokens: (session: unknown) => mockRefreshSessionTokens(session),
  sealSession: (session: unknown) => mockSealSession(session),
  sessionEndedAlready: () => mockSessionEndedAlready(),
}));

import { NextRequest } from "next/server";

import { POST } from "./route";

const refusal = (title: string, status: number) => ({ type: "about:blank", title, status });

/** The console's own heartbeat: a fetch from the page that set the cookie. */
function beat(headers: Record<string, string> = { "sec-fetch-site": "same-origin" }) {
  return POST(
    new NextRequest("http://localhost:3000/api/v1/auth/heartbeat", { method: "POST", headers }),
  );
}

describe("POST /api/v1/auth/heartbeat", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockSessionEndedAlready.mockResolvedValue(false);
    mockSealSession.mockResolvedValue("sealed_token_string");
  });

  // The cookie-clearing 401 below is exactly what makes this door a forced
  // logout if a cross-site form can reach it.
  it("refuses a cross-site request before it can touch the cookie", async () => {
    mockGetSession.mockResolvedValue(null);

    const res = await beat({ "sec-fetch-site": "cross-site" });
    expect(res.status).toBe(403);
    expect(res.cookies.get("telmoni_session")).toBeUndefined();
    expect(mockGetSession).not.toHaveBeenCalled();
  });

  it("serves a request with no Sec-Fetch-Site header, as older browsers send", async () => {
    mockGetSession.mockResolvedValue(null);

    const res = await beat({});
    expect(res.status).toBe(401);
  });

  it("returns 401 and deletes cookie when there is no active session", async () => {
    mockGetSession.mockResolvedValue(null);

    const res = await beat();
    expect(res.status).toBe(401);
    expect(await res.json()).toEqual(refusal("unauthenticated", 401));
    expect(res.cookies.get("telmoni_session")?.value).toBe("");
  });

  // ⚠ `getSession` refuses a blacklisted cookie itself, so this is the path a
  // deleted account's farewell meets. The refusal says the console ended the
  // session, and the heartbeat signs it out to the home page, not to sign-in.
  it("says a session the console ended was ended, not that it ran out", async () => {
    mockSessionEndedAlready.mockResolvedValue(true);
    mockGetSession.mockResolvedValue(null);

    const res = await beat();
    expect(res.status).toBe(401);
    expect(await res.json()).toEqual(refusal("session_ended", 401));
    expect(res.cookies.get("telmoni_session")?.value).toBe("");
  });

  it("re-seals cookie and returns expiresAt for a fresh session, without a refresh", async () => {
    const expiresAt = Date.now() + 3600_000;
    mockGetSession.mockResolvedValue({
      userId: "usr_1",
      sessionRowId: "sess_live",
      expiresAt,
      refreshToken: "rt_1",
    });

    const res = await beat();
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toEqual({ ok: true, expiresAt });
    expect(mockSealSession).toHaveBeenCalled();
    expect(mockRefreshSessionTokens).not.toHaveBeenCalled();
    expect(res.cookies.get("telmoni_session")?.value).toBe("sealed_token_string");
  });

  it("refreshes the token when nearing expiry", async () => {
    const oldExpiresAt = Date.now() + 30_000;
    const newExpiresAt = Date.now() + 3600_000;

    mockGetSession.mockResolvedValue({
      userId: "usr_1",
      sessionRowId: "sess_live",
      expiresAt: oldExpiresAt,
      refreshToken: "rt_old",
    });

    mockRefreshSessionTokens.mockResolvedValue({
      kind: "renewed",
      session: {
        userId: "usr_1",
        sessionRowId: "sess_live",
        expiresAt: newExpiresAt,
        refreshToken: "rt_new",
      },
    });

    const res = await beat();
    expect(res.status).toBe(200);
    expect(mockRefreshSessionTokens).toHaveBeenCalled();
    const body = await res.json();
    expect(body).toEqual({ ok: true, expiresAt: newExpiresAt });
  });

  // The refresh names the session row, so a revocation from another device
  // is a refused grant here rather than a separate liveness call.
  it("returns 401 and deletes the cookie when the refresh is refused", async () => {
    mockGetSession.mockResolvedValue({
      userId: "usr_1",
      sessionRowId: "sess_live",
      expiresAt: Date.now() + 30_000,
      refreshToken: "rt_old",
    });
    mockRefreshSessionTokens.mockResolvedValue({ kind: "refused" });

    const res = await beat();
    expect(res.status).toBe(401);
    expect(await res.json()).toEqual(refusal("refresh_failed", 401));
    expect(res.cookies.get("telmoni_session")?.value).toBe("");
  });

  // Auth down says nothing of the session: the cookie stands untouched, and
  // the page's heartbeat, which retries anything but a 401, asks again —
  // within auth's grace, as auth may have spent the token all the same.
  it("keeps the session and asks again soon when auth does not answer the refresh", async () => {
    mockGetSession.mockResolvedValue({
      userId: "usr_1",
      sessionRowId: "sess_live",
      expiresAt: Date.now() + 30_000,
      refreshToken: "rt_old",
    });
    mockRefreshSessionTokens.mockResolvedValue({ kind: "unreachable" });

    const res = await beat();
    expect(res.status).toBe(503);
    expect(res.headers.get("retry-after")).toBe("5");
    expect(await res.json()).toEqual(refusal("unavailable", 503));
    expect(res.cookies.get("telmoni_session")).toBeUndefined();
  });
});
