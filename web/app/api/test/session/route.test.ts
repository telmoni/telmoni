import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { unsealData } from "iron-session";

import type { SessionData } from "@/lib/auth/session";

const { fetchWithTimeout } = vi.hoisted(() => ({ fetchWithTimeout: vi.fn() }));
vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout }));

const AUTH_SECRET = "test-secret-that-is-32-chars-long!!";
// A token of the caller's choosing rides along on purpose: the door must
// seal the session auth minted, never this.
const BODY = {
  userId: "attacker",
  projectId: "someone-elses-project",
  accessToken: "a-token-of-my-choosing",
};

// What auth's test door answers for a session it opened.
function minted(sessionId = "ses_minted") {
  return {
    ok: true,
    status: 200,
    json: async () => ({
      userId: "attacker",
      accessToken: "at_minted_by_auth",
      sessionId,
      refreshToken: "rt_minted_by_auth",
      expiresIn: 900,
    }),
  };
}

async function postWith(env: Record<string, string>) {
  vi.resetModules();
  for (const [k, v] of Object.entries(env)) vi.stubEnv(k, v);
  const { POST } = await import("./route");
  return POST(
    new Request("http://localhost:3000/api/test/session", {
      method: "POST",
      body: JSON.stringify(BODY),
    }) as never,
  );
}

async function sealed(res: Awaited<ReturnType<typeof postWith>>): Promise<SessionData> {
  return unsealData<SessionData>(res.cookies.get("telmoni_session")!.value, {
    password: AUTH_SECRET,
  });
}

describe("the test-session endpoint", () => {
  beforeEach(() => {
    vi.stubEnv("AUTH_SECRET", AUTH_SECRET);
    vi.stubEnv("SERVER_URL", "http://auth");
    vi.stubEnv("SERVICE_SECRET", "s");
    fetchWithTimeout.mockReset();
    fetchWithTimeout.mockResolvedValue(minted());
  });
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it("refuses when the harness flag is absent", async () => {
    const res = await postWith({ NODE_ENV: "test" });
    expect(res.status).toBe(403);
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("refuses in production even when the flag is set", async () => {
    const res = await postWith({ NODE_ENV: "production", ALLOW_TEST_SESSION: "true" });
    expect(res.status).toBe(403);
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("refuses in production when the flag is set to anything else", async () => {
    const res = await postWith({ NODE_ENV: "production", ALLOW_TEST_SESSION: "1" });
    expect(res.status).toBe(403);
  });

  it("still works for the harness, so the refusals above are not vacuous", async () => {
    const res = await postWith({ NODE_ENV: "test", ALLOW_TEST_SESSION: "true" });
    expect(res.status).toBe(200);
    expect(res.headers.get("set-cookie") ?? "").toContain("telmoni_session=");
  });

  it("seals the session auth minted, ignoring the token the body offered", async () => {
    const before = Date.now();
    const res = await postWith({ NODE_ENV: "test", ALLOW_TEST_SESSION: "true" });
    const session = await sealed(res);

    expect(session.userId).toBe("attacker");
    expect(session.accessToken).toBe("at_minted_by_auth");
    expect(session.accessToken).not.toBe(BODY.accessToken);
    expect(session.sessionId).toBe("ses_minted");
    expect(session.refreshToken).toBe("rt_minted_by_auth");
    // A real session refreshes like any other, so it carries a real expiry.
    expect(session.expiresAt).toBeGreaterThanOrEqual(before + 900 * 1000);
    expect(session.sessionRowId).toBeNull();
  });

  it("asks auth's own door for the person named, with the service secret and no bearer", async () => {
    await postWith({ NODE_ENV: "test", ALLOW_TEST_SESSION: "true" });

    expect(fetchWithTimeout).toHaveBeenCalledTimes(1);
    const [url, init] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe("http://auth/test/session");
    expect(init.headers["x-service-secret"]).toBe("s");
    expect(init.headers.authorization).toBeUndefined();
    expect(JSON.parse(init.body)).toEqual({
      userId: "attacker",
      email: "attacker@example.test",
      firstName: null,
      lastName: null,
    });
  });

  it("seals nothing when auth would not mint the session", async () => {
    fetchWithTimeout.mockResolvedValue({ ok: false, status: 404, json: async () => ({}) });
    const res = await postWith({ NODE_ENV: "test", ALLOW_TEST_SESSION: "true" });
    expect(res.status).toBe(502);
    expect(await res.json()).toEqual({
      error: "session_not_minted",
      message: expect.stringContaining("ALLOW_TEST_SESSION"),
    });
    expect(res.cookies.get("telmoni_session")).toBeUndefined();
  });

  it("seals nothing when auth answers without the session's tokens", async () => {
    fetchWithTimeout.mockResolvedValue({ ok: true, status: 200, json: async () => ({}) });
    const res = await postWith({ NODE_ENV: "test", ALLOW_TEST_SESSION: "true" });
    expect(res.status).toBe(502);
    expect(res.cookies.get("telmoni_session")).toBeUndefined();
  });

  it("seals nothing when auth is unreachable", async () => {
    fetchWithTimeout.mockRejectedValue(new Error("ECONNREFUSED"));
    const res = await postWith({ NODE_ENV: "test", ALLOW_TEST_SESSION: "true" });
    expect(res.status).toBe(502);
    expect(res.cookies.get("telmoni_session")).toBeUndefined();
  });

  it("gives every seal the session auth opened for it", async () => {
    fetchWithTimeout
      .mockResolvedValueOnce(minted("ses_first"))
      .mockResolvedValueOnce(minted("ses_second"));
    const first = await postWith({ NODE_ENV: "test", ALLOW_TEST_SESSION: "true" });
    const second = await postWith({ NODE_ENV: "test", ALLOW_TEST_SESSION: "true" });
    expect((await sealed(first)).sessionId).toBe("ses_first");
    expect((await sealed(second)).sessionId).toBe("ses_second");
  });

  it("clears the cookie only for the harness", async () => {
    vi.resetModules();
    vi.stubEnv("NODE_ENV", "production");
    vi.stubEnv("ALLOW_TEST_SESSION", "true");
    const { DELETE } = await import("./route");
    expect((await DELETE()).status).toBe(403);
  });
});
