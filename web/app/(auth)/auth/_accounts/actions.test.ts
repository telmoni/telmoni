import { beforeEach, describe, expect, it, vi } from "vitest";

import { errorOf } from "@/lib/action-result";

// The pending sign-in the door sealed, its state `st8` and its verifier the RFC 7636 example's.
const pending = { value: "sealed-pkce" as string | undefined };
vi.mock("next/headers", () => ({
  headers: async () => new Headers({ "x-forwarded-for": "203.0.113.9" }),
  cookies: async () => ({ get: () => (pending.value ? { value: pending.value } : undefined) }),
}));
vi.mock("@/lib/auth/session", () => ({
  PKCE_COOKIE: "telmoni_pkce",
  unsealPkce: vi.fn(async () => ({
    state: "st8",
    verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
    nonce: "n".repeat(43),
    returnTo: "/console",
  })),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  clientKeyFromHeaders: (headers: Headers, scope: string) =>
    `bfrl:${scope}:${headers.get("x-forwarded-for")}`,
  sessionKey: (session: { userId: string }, action: string) => `bfrl:${session.userId}:${action}`,
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async (res: Response) => {
    const body = (await res.json().catch(() => null)) as { detail?: string; title?: string } | null;
    return { message: body?.title ?? `request failed (${res.status})`, problem: body };
  }),
}));
vi.mock("@/lib/env", () => ({
  env: { SERVER_URL: "http://auth.test", SERVICE_SECRET: "svc" },
}));
vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));

import { NextResponse } from "next/server";

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit } from "@/lib/api/rate-limit";
import { unsealPkce } from "@/lib/auth/session";
import { getServerSession } from "@/lib/server/session";

import {
  deviceDecisionAction,
  forgotPasswordAction,
  resetPasswordAction,
  signInAction,
  signUpAction,
  verifyEmailAction,
} from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);
const limiter = vi.mocked(rateLimit);

const problem = (status: number, detail: string) =>
  new Response(JSON.stringify({ type: "/errors/x", title: "refused", status, detail }), {
    status,
    headers: { "content-type": "application/problem+json" },
  });

function sent(): { url: string; init: { method: string; headers: Record<string, string>; body: string } } {
  const [url, init] = fetchMock.mock.calls[0]! as [string, { method: string; headers: Record<string, string>; body: string }];
  return { url, init };
}

beforeEach(() => {
  vi.clearAllMocks();
  limiter.mockResolvedValue(null);
});

describe("signInAction", () => {
  it("posts the credentials with the service secret and answers the callback URL", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ code: "code_1" }), { status: 200 }));
    const res = await signInAction({ email: "ada@example.com", password: "hunter22", state: "st8" });
    expect(res).toEqual({ ok: true, next: "/auth/callback?code=code_1&state=st8" });
    const { url, init } = sent();
    expect(url).toBe("http://auth.test/internal/v1/auth/password/sign-in");
    expect(init.headers["x-service-secret"]).toBe("svc");
    expect(init.headers).not.toHaveProperty("authorization");
    expect(JSON.parse(init.body)).toEqual({
      email: "ada@example.com",
      password: "hunter22",
      codeChallenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
    });
  });

  // The code auth mints is bound to the browser holding the sealed verifier.
  it("refuses a sign-in no door started, before any call", async () => {
    pending.value = undefined;
    const res = await signInAction({ email: "ada@example.com", password: "hunter22", state: "st8" });
    expect(errorOf(res)).toMatch(/expired/);
    expect(fetchMock).not.toHaveBeenCalled();
    pending.value = "sealed-pkce";
    const other = await signInAction({ email: "ada@example.com", password: "hunter22", state: "other" });
    expect(errorOf(other)).toMatch(/expired/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // Its callback would hand the form's code, and the verifier, to the provider.
  it("refuses a sign-in that Continue with… took over, before any call", async () => {
    vi.mocked(unsealPkce).mockResolvedValueOnce({
      state: "st8",
      verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
      nonce: "n".repeat(43),
      returnTo: "/console",
      provider: "external",
    });
    const res = await signInAction({ email: "ada@example.com", password: "hunter22", state: "st8" });
    expect(errorOf(res)).toMatch(/expired/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // A wrong password and an unknown address are the same answer, here as at auth.
  it("says only that the pair was wrong on a 401", async () => {
    fetchMock.mockResolvedValue(problem(401, "unauthenticated"));
    const res = await signInAction({ email: "ada@example.com", password: "nope", state: "st8" });
    expect(errorOf(res)).toBe("Wrong email address or password.");
    expect(res).not.toHaveProperty("next");
  });

  it("relays auth's own explanation for any other refusal", async () => {
    fetchMock.mockResolvedValue(problem(403, "verify your email address first"));
    const res = await signInAction({ email: "ada@example.com", password: "hunter22", state: "st8" });
    expect(errorOf(res)).toBe("verify your email address first");
  });

  // A domain held to single sign-on is told so, and the form gives way to the provider.
  it("says a domain held to single sign-on signs in at its provider", async () => {
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({ type: "/errors/auth/sso-required", title: "single sign-on required", status: 403 }),
        { status: 403, headers: { "content-type": "application/problem+json" } },
      ),
    );
    const res = await signInAction({ email: "ada@corp.example", password: "hunter22", state: "st8" });
    expect(res).toMatchObject({ ok: false, sso: true });
    expect(errorOf(res)).toMatch(/identity provider/);
    expect(res).not.toHaveProperty("next");
  });

  it("is unavailable, not broken, when the lane is absent or auth is down", async () => {
    fetchMock.mockResolvedValue(problem(404, "no such lane"));
    expect(errorOf(await signInAction({ email: "a@b.co", password: "x", state: "st8" }))).toMatch(/unavailable/);
    fetchMock.mockResolvedValue(null);
    expect(errorOf(await signInAction({ email: "a@b.co", password: "x", state: "st8" }))).toMatch(/unavailable/);
  });

  it("refuses an empty form and a metered caller before any call", async () => {
    expect(errorOf(await signInAction({ email: "not-an-address", password: "x", state: "s" }))).toMatch(/email address and password/);
    limiter.mockResolvedValue(new NextResponse(null, { status: 429 }));
    expect(errorOf(await signInAction({ email: "a@b.co", password: "x", state: "s" }))).toMatch(/too many/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("signUpAction", () => {
  it("posts the account and the optional names, and waits for the mail on a 202", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ sent: true }), { status: 202 }));
    const res = await signUpAction({
      email: "ada@example.com",
      password: "correct horse battery staple",
      givenName: "Ada",
      familyName: "",
      state: "st8",
    });
    expect(res).toEqual({ ok: true });
    const { url, init } = sent();
    expect(url).toBe("http://auth.test/internal/v1/auth/password/sign-up");
    expect(JSON.parse(init.body)).toEqual({
      email: "ada@example.com",
      password: "correct horse battery staple",
      codeChallenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
      givenName: "Ada",
    });
  });

  // With no address to confirm, auth answers the code that opens the
  // session, and the browser goes to the callback with it as after a sign-in.
  it("answers the callback URL when auth signs the new account in at once", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ code: "code_9" }), { status: 200 }));
    const res = await signUpAction({
      email: "ada@example.com",
      password: "correct horse battery staple",
      state: "st8",
    });
    expect(res).toEqual({ ok: true, next: "/auth/callback?code=code_9&state=st8" });
  });

  it("relays auth's refusal of an address that has an account, or of an uninvited one", async () => {
    fetchMock.mockResolvedValue(problem(409, "that address already has an account; sign in instead"));
    let res = await signUpAction({ email: "ada@example.com", password: "correct horse battery staple", state: "st8" });
    expect(errorOf(res)).toMatch(/already has an account/);
    fetchMock.mockResolvedValue(problem(403, "sign-ups are closed; ask somebody to invite you"));
    res = await signUpAction({ email: "ada@example.com", password: "correct horse battery staple", state: "st8" });
    expect(errorOf(res)).toMatch(/sign-ups are closed/);
  });

  it("holds the password to the floor auth holds it to, before any call", async () => {
    const res = await signUpAction({ email: "ada@example.com", password: "short", state: "st8" });
    expect(errorOf(res)).toBe("Use at least 8 characters.");
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("the link actions", () => {
  it("verifyEmailAction spends the link's two halves", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    expect(await verifyEmailAction({ userId: "user_1", token: "tok" })).toEqual({ ok: true });
    const { url, init } = sent();
    expect(url).toBe("http://auth.test/internal/v1/auth/password/verify");
    expect(JSON.parse(init.body)).toEqual({ userId: "user_1", token: "tok" });
  });

  it("verifyEmailAction relays a stale link's refusal", async () => {
    fetchMock.mockResolvedValue(problem(400, "that link is invalid or has expired"));
    expect(errorOf(await verifyEmailAction({ userId: "user_1", token: "tok" }))).toMatch(/expired/);
  });

  it("forgotPasswordAction says nothing about whether the address has an account", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ sent: true }), { status: 202 }));
    expect(await forgotPasswordAction({ email: "ada@example.com" })).toEqual({ ok: true });
    expect(sent().url).toBe("http://auth.test/internal/v1/auth/password/forgot");
  });

  it("resetPasswordAction posts the link and the new password", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    expect(
      await resetPasswordAction({ userId: "user_1", token: "tok", password: "correct horse battery staple" }),
    ).toEqual({ ok: true });
    const { url, init } = sent();
    expect(url).toBe("http://auth.test/internal/v1/auth/password/reset");
    expect(JSON.parse(init.body)).toEqual({
      userId: "user_1",
      token: "tok",
      password: "correct horse battery staple",
    });
  });
});

describe("deviceDecisionAction", () => {
  beforeEach(() => {
    vi.mocked(getServerSession).mockResolvedValue({
      userId: "user_1",
      email: "ada@example.com",
      accessToken: "at_1",
    } as never);
  });

  it("approves as the signed-in person, under their bearer", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    expect(await deviceDecisionAction({ userCode: "wdjb-mjht", decision: "approve" })).toEqual({ ok: true });
    const { url, init } = sent();
    expect(url).toBe("http://auth.test/internal/v1/auth/device/approve");
    expect(init.headers.authorization).toBe("Bearer at_1");
    expect(JSON.parse(init.body)).toEqual({ userCode: "wdjb-mjht" });
  });

  it("denies on the deny lane", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    await deviceDecisionAction({ userCode: "WDJB-MJHT", decision: "deny" });
    expect(sent().url).toBe("http://auth.test/internal/v1/auth/device/deny");
  });

  it("needs a session, and names a code nobody is waiting on", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null);
    expect(errorOf(await deviceDecisionAction({ userCode: "x", decision: "approve" }))).toMatch(/sign in/);
    vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1", accessToken: "at_1" } as never);
    fetchMock.mockResolvedValue(problem(404, "no device is waiting for that code"));
    expect(errorOf(await deviceDecisionAction({ userCode: "x", decision: "approve" }))).toMatch(/no device/);
  });
});
