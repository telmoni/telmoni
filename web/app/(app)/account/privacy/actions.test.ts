import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "problem" })),
}));
// Mocked so "the cookie is left alone" is an assertion rather than an
// absence: signing out writes cookies, and a cookie write re-renders the page
// before the farewell is on screen.
vi.mock("@/lib/auth/session", () => ({
  destroySession: vi.fn(async () => {}),
}));
vi.mock("@/lib/auth/session-blacklist", () => ({
  blacklistSession: vi.fn(async () => {}),
  isSessionBlacklisted: vi.fn(async () => false),
}));
vi.mock("@/lib/env", () => ({
  env: {
    SERVER_URL: "http://auth.test",
    SERVICE_SECRET: "secret",
  },
}));
// Restoring an organization points the console at it; that write is the
// assertion, so it is a spy rather than a real cookie jar.
vi.mock("@/lib/server/cookies", () => ({
  setActiveOrganizationCookie: vi.fn(async () => {}),
}));
// `/me` is the one thing stubbed: the real header builders make what reaches
// auth, and that is the assertion under test.
vi.mock("@/lib/server/entities/organization", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/organization")>()),
  getServerContext: vi.fn(),
}));

import { getServerSession } from "@/lib/server/session";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { destroySession } from "@/lib/auth/session";
import { setActiveOrganizationCookie } from "@/lib/server/cookies";
import { getServerContext } from "@/lib/server/entities/organization";

import {
  deleteAccountAction,
  requestAccountDeletionCodeAction,
  restoreOrganizationAction,
  revokeSessionAction,
  setAnalyticsPreferenceAction,
} from "./actions";
import { blacklistSession, isSessionBlacklisted } from "@/lib/auth/session-blacklist";
import { rateLimit } from "@/lib/api/rate-limit";

const fetchMock = vi.mocked(tryFetchWithTimeout);

const UNRESOLVED = "Couldn't resolve your account right now. Try again in a moment.";
const UNREACHABLE = "The organization service is unreachable. Try again.";

function sentHeaders(): Record<string, string> {
  return (fetchMock.mock.calls[0]![1] as { headers: Record<string, string> }).headers;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_1",
    email: "user@example.test",
    accessToken: "at_1",
    sessionRowId: "sess_1",
  } as never);
  vi.mocked(isSessionBlacklisted).mockResolvedValue(false);
  vi.mocked(rateLimit).mockResolvedValue(null as never);
  // Standing in somebody else's organization. These lanes act on the ACCOUNT
  // wherever the console is, and send that organization along for auth to
  // check the person is in and record against.
  standIn("org_active");
});

/// What `/me` answers: the person in `organizationId` as a member, or in no
/// organization at all.
function standIn(organizationId: string | null) {
  vi.mocked(getServerContext).mockResolvedValue({
    organizations: organizationId ? [{ organizationId, role: "member" }] : [],
    activeOrganizationId: organizationId,
  } as never);
}

describe("requestAccountDeletionCodeAction", () => {
  it("asks auth to mail the account's code, under the session's bearer", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ sent: true }), { status: 200 }));
    const res = await requestAccountDeletionCodeAction();
    expect(res).toEqual({ ok: true });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/me/deletion-code",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-service-secret": "secret",
        }),
      }),
    );
    expect(sentHeaders()).not.toHaveProperty("x-user-id");
    expect(sentHeaders()).not.toHaveProperty("x-user-email");
  });

  // ⚠ **The account's own lane names no organization**, so nothing about
  // where the person stands can stop it: somebody in none — sign-ups closed,
  // or behind the beta wall — reaches it from the account screen, and the
  // privacy policy promises they can.
  it("reaches somebody in no organization, and names none", async () => {
    standIn(null);
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ sent: true }), { status: 202 }));
    expect(await requestAccountDeletionCodeAction()).toEqual({ ok: true });
    expect(sentHeaders()).not.toHaveProperty("x-organization-id");
  });

  it("reports an unreachable service and a refusal distinctly", async () => {
    fetchMock.mockResolvedValue(null as never);
    expect(await requestAccountDeletionCodeAction()).toEqual({ error: UNREACHABLE });

    fetchMock.mockResolvedValue(new Response(null, { status: 403 }));
    expect(await requestAccountDeletionCodeAction()).toEqual({ error: "problem" });
  });
});

describe("deleteAccountAction", () => {
  it("rejects a code that is not six digits, without calling auth", async () => {
    for (const bad of ["abc", "12345", "1234567", "12345a", ""]) {
      expect(await deleteAccountAction(bad), bad).toEqual({
        error: "Enter the 6-digit code from your email.",
      });
    }
    expect(fetchMock).not.toHaveBeenCalled();
    expect(blacklistSession).not.toHaveBeenCalled();
    expect(destroySession).not.toHaveBeenCalled();
  });

  it("deletes the account, then ends this browser's session", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ deleted: true }), { status: 200 }));
    const res = await deleteAccountAction("123456");
    expect(res).toEqual({ ok: true, deleted: true });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/me",
      expect.objectContaining({
        method: "DELETE",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "content-type": "application/json",
        }),
        body: JSON.stringify({ code: "123456" }),
      }),
    );
    expect(sentHeaders()).not.toHaveProperty("x-user-email");
    expect(sentHeaders()).not.toHaveProperty("x-organization-id");
    expect(blacklistSession).toHaveBeenCalledTimes(1);
    expect(blacklistSession).toHaveBeenCalledWith("sess_1");
    expect(
      vi.mocked(blacklistSession).mock.invocationCallOrder[0]!,
      "the session ended before auth confirmed the deletion",
    ).toBeGreaterThan(fetchMock.mock.invocationCallOrder[0]!);
  });

  // ⚠ **The farewell never showed while this signed out.** `destroySession`
  // deletes the cookies, a cookie write re-renders the page inside the
  // action's response, and the (app) layout — finding no session — redirected
  // to sign-out before "Sorry to see you go" was on screen. The blacklist is a
  // Redis write, which re-renders nothing; the farewell's own timer and button
  // go to `/auth/logout` for the full sign-out.
  it("leaves the cookie to the farewell's sign-out, so the farewell can render", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ deleted: true }), { status: 200 }));
    expect(await deleteAccountAction("123456")).toEqual({ ok: true, deleted: true });
    expect(destroySession).not.toHaveBeenCalled();
  });

  // A session sealed without a row id has nothing auth or Redis could name.
  it("ends nothing when the session names neither a row nor a sid, and still succeeds", async () => {
    vi.mocked(getServerSession).mockResolvedValue({
      userId: "user_1",
      email: "user@example.test",
      accessToken: "at_1",
      sessionRowId: null,
      sessionId: null,
    } as never);
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ deleted: true }), { status: 200 }));
    expect(await deleteAccountAction("123456")).toEqual({ ok: true, deleted: true });
    expect(blacklistSession).toHaveBeenCalledWith(null);
    expect(destroySession).not.toHaveBeenCalled();
  });

  // Auth answers 202 when the erasure goes on in the background. The access
  // ends now either way, so the session does too.
  it("ends the session of a deletion still underway, and says it is not erased yet", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ deleted: false }), { status: 202 }));
    expect(await deleteAccountAction("123456")).toEqual({ ok: true, deleted: false });
    expect(blacklistSession).toHaveBeenCalledWith("sess_1");
  });

  // ⚠ Auth refuses while the person owns an organization anybody else is in,
  // and names each one. That answer IS the next step — hand it over, or empty
  // it — so it reaches the form word for word, and the session survives it.
  it("surfaces what auth says blocks it, and keeps the session", async () => {
    const { extractProblem: fromProblem } =
      await vi.importActual<typeof import("@/lib/api/fetch")>("@/lib/api/fetch");
    vi.mocked(extractProblem).mockImplementationOnce(fromProblem);
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({
          type: "/errors/auth/conflict",
          title: "conflict",
          status: 409,
          detail:
            "you own organizations other people are in: Acme. Transfer ownership of each, or remove everyone else from it, then delete your account.",
        }),
        { status: 409, headers: { "content-type": "application/problem+json" } },
      ),
    );

    const res = await deleteAccountAction("123456");
    expect(res.ok).toBeUndefined();
    expect(res.error).toMatch(/other people are in: Acme\. Transfer ownership/);
    expect(blacklistSession).not.toHaveBeenCalled();
    expect(destroySession).not.toHaveBeenCalled();
  });

  it("deletes the account of somebody in no organization", async () => {
    standIn(null);
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ deleted: true }), { status: 200 }));
    expect(await deleteAccountAction("123456")).toEqual({ ok: true, deleted: true });
    expect(blacklistSession).toHaveBeenCalledWith("sess_1");
  });

  // ⚠ A sign-in whose probe never recorded a session row still ends under
  // the provider's session id: without that key the farewell's sign-out
  // could not tell the session was over, and took the provider's page.
  it("ends a row-less session under the session id the exchange answered with", async () => {
    vi.mocked(getServerSession).mockResolvedValue({
      userId: "user_1",
      email: "user@example.test",
      accessToken: "at_1",
      sessionRowId: null,
      sessionId: "sid_probe_failed",
    } as never);
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ deleted: true }), { status: 200 }));
    expect(await deleteAccountAction("123456")).toEqual({ ok: true, deleted: true });
    expect(blacklistSession).toHaveBeenCalledWith("sid_probe_failed");
  });

  it("reports an unreachable service rather than ending the session", async () => {
    fetchMock.mockResolvedValue(null as never);
    expect(await deleteAccountAction("123456")).toEqual({ error: UNREACHABLE });
    expect(blacklistSession).not.toHaveBeenCalled();
    expect(destroySession).not.toHaveBeenCalled();
  });
});

describe("restoreOrganizationAction", () => {
  // The organization named is the closed one being brought back, never the
  // one the console is standing in: `/me` lists it apart, and auth checks
  // the owner under its lock.
  it("asks auth to restore the organization named, and points the console at it", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    expect(await restoreOrganizationAction("org_closed")).toEqual({ ok: true });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/organization/restore",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-service-secret": "secret",
          "x-organization-id": "org_closed",
        }),
      }),
    );
    expect(sentHeaders()).not.toHaveProperty("x-user-id");
    expect(setActiveOrganizationCookie).toHaveBeenCalledWith("org_closed");
  });

  // Reachable from the account screen too: whoever just deleted their only
  // organization stands in none, and the way back must not need one.
  it("needs no organization to stand in", async () => {
    standIn(null);
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    expect(await restoreOrganizationAction("org_closed")).toEqual({ ok: true });
  });

  it("refuses something that is not an organization id without asking auth", async () => {
    for (const bad of ["", "user_1", "org_", "org_x;drop", "../me"]) {
      expect(await restoreOrganizationAction(bad), bad).toEqual({
        error: "That is not an organization.",
      });
    }
    expect(fetchMock).not.toHaveBeenCalled();
    expect(setActiveOrganizationCookie).not.toHaveBeenCalled();
  });

  it("surfaces auth's refusal and points the console nowhere", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 409 }));
    expect(await restoreOrganizationAction("org_closed")).toEqual({ error: "problem" });
    expect(setActiveOrganizationCookie).not.toHaveBeenCalled();

    fetchMock.mockResolvedValue(null as never);
    expect(await restoreOrganizationAction("org_closed")).toEqual({ error: UNREACHABLE });
  });

  it("refuses a caller over the limit without asking auth", async () => {
    vi.mocked(rateLimit).mockResolvedValue({ retryAfter: 1 } as never);
    expect(await restoreOrganizationAction("org_closed")).toEqual({
      error: "Too many attempts. Try again in an hour.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("revokeSessionAction", () => {
  it("ends the session under the organization the console is standing in", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));

    const res = await revokeSessionAction("sess_abc");

    expect(res).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/auth/sessions/sess_abc/revoke",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": "org_active",
          "x-service-secret": "secret",
        }),
      }),
    );
    expect(sentHeaders()).not.toHaveProperty("x-user-id");
  });

  it("encodes the session id into the path", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    await revokeSessionAction("sess/../other");
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/auth/sessions/sess%2F..%2Fother/revoke",
      expect.anything(),
    );
  });

  it("blacklists the session once auth has revoked it, and not before", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    await revokeSessionAction("sess_abc");
    expect(blacklistSession).toHaveBeenCalledWith("sess_abc");

    vi.clearAllMocks();
    fetchMock.mockResolvedValue(new Response(null, { status: 500 }));
    expect(await revokeSessionAction("sess_abc")).toEqual({ error: "problem" });
    expect(blacklistSession).not.toHaveBeenCalled();
  });

  it("reports an unreachable service rather than claiming success", async () => {
    fetchMock.mockResolvedValue(null as never);
    expect(await revokeSessionAction("sess_abc")).toEqual({ error: UNREACHABLE });
    expect(blacklistSession).not.toHaveBeenCalled();
  });

  it("refuses without calling auth when it cannot tell where the caller stands", async () => {
    vi.mocked(getServerContext).mockResolvedValue(null);
    expect(await revokeSessionAction("sess_abc")).toEqual({ error: UNRESOLVED });
    expect(fetchMock).not.toHaveBeenCalled();
    expect(blacklistSession).not.toHaveBeenCalled();
  });

  // ⚠ Cutting off a stolen session must not depend on belonging somewhere:
  // somebody in no organization names none, and auth logs the revocation.
  it("revokes a session for somebody in no organization, naming none", async () => {
    standIn(null);
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    expect(await revokeSessionAction("sess_abc")).toEqual({ error: null });
    expect(sentHeaders()).not.toHaveProperty("x-organization-id");
    expect(blacklistSession).toHaveBeenCalledWith("sess_abc");
  });

  it("refuses a session that is not live, and a caller over the limit", async () => {
    vi.mocked(isSessionBlacklisted).mockResolvedValue(true as never);
    expect(await revokeSessionAction("sess_abc")).toEqual({
      error: "Your session expired — sign in again.",
    });
    expect(isSessionBlacklisted).toHaveBeenCalledWith("sess_1");
    expect(fetchMock).not.toHaveBeenCalled();

    vi.mocked(isSessionBlacklisted).mockResolvedValue(false as never);
    vi.mocked(rateLimit).mockResolvedValue("limited" as never);
    expect(await revokeSessionAction("sess_abc")).toEqual({
      error: "Too many requests — slow down a moment.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

// The consent is the PERSON's, whichever organization they are standing in.
describe("setAnalyticsPreferenceAction", () => {
  it("records the consent on the account, under the session's bearer", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    expect(await setAnalyticsPreferenceAction(true)).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/me/analytics",
      expect.objectContaining({
        method: "PUT",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": "org_active",
          "content-type": "application/json",
        }),
        body: JSON.stringify({ opt_in: true }),
      }),
    );
  });

  it("refuses without calling auth when it cannot tell where the caller stands", async () => {
    vi.mocked(getServerContext).mockResolvedValue(null);
    expect(await setAnalyticsPreferenceAction(false)).toEqual({ error: UNRESOLVED });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("records the consent of somebody in no organization, naming none", async () => {
    standIn(null);
    fetchMock.mockResolvedValue(new Response(null, { status: 200 }));
    expect(await setAnalyticsPreferenceAction(true)).toEqual({ error: null });
    expect(sentHeaders()).not.toHaveProperty("x-organization-id");
  });
});
