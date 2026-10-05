import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/auth/session", () => ({
  writeSessionCookie: vi.fn(async () => undefined),
}));
vi.mock("@/lib/auth/session-blacklist", () => ({
  blacklistSession: vi.fn(async () => undefined),
  isSessionBlacklisted: vi.fn(async () => false),
}));
vi.mock("next/cache", () => ({
  revalidatePath: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "problem" })),
}));
vi.mock("@/lib/env", () => ({
  env: {
    SERVER_URL: "http://auth.test",
    SERVICE_SECRET: "secret",
  },
}));
// `/me` is the one thing stubbed: the real `accountHeaders` builds what reaches
// auth, and that is the assertion under test.
vi.mock("@/lib/server/entities/organization", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/organization")>()),
  getServerContext: vi.fn(),
}));

import { revalidatePath } from "next/cache";

import { getServerSession } from "@/lib/server/session";
import { writeSessionCookie } from "@/lib/auth/session";
import { blacklistSession, isSessionBlacklisted } from "@/lib/auth/session-blacklist";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { getServerContext } from "@/lib/server/entities/organization";

import {
  confirmEmailChangeAction,
  requestEmailChangeAction,
  requestPasswordResetAction,
  setDefaultOrganizationAction,
} from "./actions";

const UNRESOLVED = "Couldn't resolve your account right now. Try again in a moment.";

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_1",
    email: "k@example.test",
    authMethod: "password",
    sessionRowId: "sess_1",
    accessToken: "at_1",
  } as never);
  vi.mocked(rateLimit).mockResolvedValue(null as never);
  vi.mocked(isSessionBlacklisted).mockResolvedValue(false);
  vi.mocked(writeSessionCookie).mockResolvedValue(undefined);
  // Standing in somebody else's organization: an account lane acts on the
  // person wherever the console is, and sends that organization along.
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

function sentHeaders(): Record<string, string> {
  return (vi.mocked(tryFetchWithTimeout).mock.calls[0]![1] as { headers: Record<string, string> })
    .headers;
}

// Auth answers the confirm with the address the identity provider reported.
const confirmed = (email = "new@example.test") =>
  new Response(JSON.stringify({ email, sessionsRevoked: 2 }), {
    status: 200,
    headers: { "content-type": "application/json" },
  });

describe("requestPasswordResetAction", () => {
  it("asks auth for the caller's own reset, sending no address and no password", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(
      new Response(null, { status: 202 }),
    );

    expect(await requestPasswordResetAction()).toEqual({ error: null });

    const [url, init] = vi.mocked(tryFetchWithTimeout).mock.calls[0]!;
    expect(url).toBe("http://auth.test/internal/me/password-reset");
    expect(init).toEqual(
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
    expect(sentHeaders()).not.toHaveProperty("x-user-email");
    expect(init).not.toHaveProperty("body");
  });

  // ⚠ The path named the organization by the caller's user id, and had to be
  // encoded against one shaped like a traversal. The bearer names the person
  // now, so the path is fixed whatever the session holds.
  it("puts no id in the path, whatever the session holds", async () => {
    vi.mocked(getServerSession).mockResolvedValue({
      userId: "user/../evil",
      email: "k@example.test",
    } as never);
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(
      new Response(null, { status: 202 }),
    );

    await requestPasswordResetAction();
    expect(vi.mocked(tryFetchWithTimeout).mock.calls[0]![0]).toBe(
      "http://auth.test/internal/me/password-reset",
    );
  });

  it("refuses without calling auth when it cannot tell where the caller stands", async () => {
    vi.mocked(getServerContext).mockResolvedValue(null);
    expect(await requestPasswordResetAction()).toEqual({ error: UNRESOLVED });
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  // Sign-ups closed after they left their last organization: the account is
  // still theirs, and so is its password.
  it("reaches somebody in no organization, and names none", async () => {
    standIn(null);
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(new Response(null, { status: 202 }));
    expect(await requestPasswordResetAction()).toEqual({ error: null });
    expect(sentHeaders()).not.toHaveProperty("x-organization-id");
    expect(sentHeaders().authorization).toBe("Bearer at_1");
  });

  it("refuses a caller over the limit without calling auth", async () => {
    vi.mocked(rateLimit).mockResolvedValue("limited" as never);
    expect(await requestPasswordResetAction()).toEqual({
      error: "Too many requests — try again in an hour.",
    });
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it("refuses a caller with no session without calling auth", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null as never);
    expect(await requestPasswordResetAction()).toEqual({
      error: "Your session expired — sign in again.",
    });
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it("refuses a session that was ended from another device", async () => {
    vi.mocked(isSessionBlacklisted).mockResolvedValue(true);
    expect(await requestPasswordResetAction()).toEqual({
      error: "Your session expired — sign in again.",
    });
    expect(isSessionBlacklisted).toHaveBeenCalledWith("sess_1");
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
    expect(rateLimit).not.toHaveBeenCalled();
  });

  it("reports an unreachable service and a refusal distinctly", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(null as never);
    expect(await requestPasswordResetAction()).toEqual({
      error: "The organization service is unreachable. Try again.",
    });

    vi.mocked(tryFetchWithTimeout).mockResolvedValue(
      new Response(null, { status: 502 }),
    );
    expect(await requestPasswordResetAction()).toEqual({ error: "problem" });
  });
});

describe("requestEmailChangeAction", () => {
  it("asks auth to open a change, carrying the new address in the body", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(
      new Response(null, { status: 202 }),
    );

    expect(await requestEmailChangeAction("new@example.test")).toEqual({
      error: null,
      email: "new@example.test",
    });

    const [url, init] = vi.mocked(tryFetchWithTimeout).mock.calls[0]!;
    expect(url).toBe("http://auth.test/internal/me/email-change");
    expect(init).toEqual(
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ newEmail: "new@example.test" }),
      }),
    );
  });

  // The bearer names the account, so no address rides in a header any more —
  // the current one went with `x-user-email`. The new address stays in the
  // body: putting it anywhere auth reads as the account's would be the first
  // step to a lane that mails a caller-supplied destination.
  it("sends no address in a header, and the new one only in the body", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(
      new Response(null, { status: 202 }),
    );
    await requestEmailChangeAction("new@example.test");

    const headers = sentHeaders();
    expect(headers).not.toHaveProperty("x-user-email");
    expect(Object.values(headers)).not.toContain("new@example.test");
    expect(Object.values(headers)).not.toContain("k@example.test");
    expect(headers.authorization).toBe("Bearer at_1");
    expect(headers["x-organization-id"]).toBe("org_active");
    expect(headers["x-service-secret"]).toBe("secret");
    expect(headers).not.toHaveProperty("x-user-id");
  });

  it("puts no id in the path, whatever the session holds", async () => {
    vi.mocked(getServerSession).mockResolvedValue({
      userId: "user/../evil",
      email: "k@example.test",
      authMethod: "password",
    } as never);
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(
      new Response(null, { status: 202 }),
    );

    await requestEmailChangeAction("new@example.test");
    expect(vi.mocked(tryFetchWithTimeout).mock.calls[0]![0]).toBe(
      "http://auth.test/internal/me/email-change",
    );
  });

  it("refuses without calling auth when it cannot tell where the caller stands", async () => {
    vi.mocked(getServerContext).mockResolvedValue(null);
    expect(await requestEmailChangeAction("new@example.test")).toEqual({
      error: UNRESOLVED,
    });
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it("normalises the address before it leaves, and answers with what it sent", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(
      new Response(null, { status: 202 }),
    );

    expect(await requestEmailChangeAction("  New@Example.TEST  ")).toEqual({
      error: null,
      email: "new@example.test",
    });
    expect(
      vi.mocked(tryFetchWithTimeout).mock.calls[0]![1],
    ).toEqual(
      expect.objectContaining({
        body: JSON.stringify({ newEmail: "new@example.test" }),
      }),
    );
  });

  it("refuses a malformed address without calling auth", async () => {
    for (const bad of ["", "nope", "a@b", "two @spaces.test"]) {
      expect(await requestEmailChangeAction(bad), bad).toEqual({
        error: "Enter a valid email address.",
      });
    }
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  // Case-insensitively, because the column stores lowercase and an address that
  // differs only in case is the same address. Refusing here saves two emails.
  it("refuses the address the account already has", async () => {
    for (const same of ["k@example.test", "K@Example.TEST"]) {
      expect(await requestEmailChangeAction(same), same).toEqual({
        error: "That is already the address on this account.",
      });
    }
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  // ⚠ The most important test in this file. A Server Action is a public
  // endpoint, and auth CANNOT enforce this rule — the sign-in method never
  // reaches it. Without the gate, a provider account changes its address at the
  // identity provider, and the provider puts the old one back at its next
  // sign-in.
  it("refuses an account whose provider holds the address, without calling auth", async () => {
    for (const method of ["google", "microsoft", "sso", "magic_link", null]) {
      vi.mocked(getServerSession).mockResolvedValue({
        userId: "user_1",
        email: "k@example.test",
        authMethod: method,
      } as never);
      const res = await requestEmailChangeAction("new@example.test");
      expect(res.error, String(method)).toMatch(/held by your identity provider/);
    }
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  // The budget is the relay guard: this lane mails an address the caller typed,
  // unlike its siblings which mail the address on the account. Loosening it to
  // make "Resend codes" feel comfortable is the change this pins against.
  it("caps issuing at three an hour, on its own scope", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(
      new Response(null, { status: 202 }),
    );
    await requestEmailChangeAction("new@example.test");
    expect(sessionKey).toHaveBeenCalledWith(
      expect.anything(),
      "email:change-code",
    );
    expect(rateLimit).toHaveBeenCalledWith("k", {
      limit: 3,
      windowMs: 3_600_000,
    });

    vi.mocked(rateLimit).mockResolvedValue("limited" as never);
    expect(await requestEmailChangeAction("new@example.test")).toEqual({
      error: "Too many requests — try again in an hour.",
    });
  });

  it("refuses a dead session before spending a rate-limit slot", async () => {
    vi.mocked(isSessionBlacklisted).mockResolvedValue(true);
    expect(await requestEmailChangeAction("new@example.test")).toEqual({
      error: "Your session expired — sign in again.",
    });
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
    expect(rateLimit).not.toHaveBeenCalled();
  });

  it("reports an unreachable service and a refusal distinctly", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(null as never);
    expect(await requestEmailChangeAction("new@example.test")).toEqual({
      error: "The organization service is unreachable. Try again.",
    });

    vi.mocked(tryFetchWithTimeout).mockResolvedValue(
      new Response(null, { status: 409 }),
    );
    expect(await requestEmailChangeAction("new@example.test")).toEqual({
      error: "problem",
    });
  });
});

describe("confirmEmailChangeAction", () => {
  it("spends both codes and answers with the address the provider reported", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(confirmed());

    expect(await confirmEmailChangeAction("111111", "222222")).toEqual({
      error: null,
      email: "new@example.test",
    });

    const [url, init] = vi.mocked(tryFetchWithTimeout).mock.calls[0]!;
    expect(url).toBe("http://auth.test/internal/me/email-change/confirm");
    expect(init).toEqual(
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ currentCode: "111111", newCode: "222222" }),
      }),
    );
    // Where auth records the change, once it has checked the person is in it;
    // the account itself is the bearer's, and no address is asserted beside it.
    expect(sentHeaders()["x-organization-id"]).toBe("org_active");
    expect(sentHeaders().authorization).toBe("Bearer at_1");
    expect(sentHeaders()).not.toHaveProperty("x-user-email");
  });

  it("refuses without calling auth when it cannot tell where the caller stands", async () => {
    vi.mocked(getServerContext).mockResolvedValue(null);
    expect(await confirmEmailChangeAction("111111", "222222")).toEqual({
      error: UNRESOLVED,
    });
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
    expect(blacklistSession).not.toHaveBeenCalled();
  });

  // No chain is theirs to be recorded on, so none is named; auth logs the
  // change, and refuses a missing header from anybody who is in one.
  it("moves the address of somebody in no organization, naming none", async () => {
    standIn(null);
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(confirmed());
    expect(await confirmEmailChangeAction("111111", "222222")).toEqual({
      error: null,
      email: "new@example.test",
    });
    expect(sentHeaders()).not.toHaveProperty("x-organization-id");
    expect(blacklistSession).toHaveBeenCalledWith("sess_1");
  });

  it("will not post anything but two six-digit codes", async () => {
    for (const [a, b] of [
      ["12345", "222222"],
      ["222222", "12345"],
      ["1234567", "222222"],
      ["12345a", "222222"],
      ["", ""],
    ]) {
      expect(await confirmEmailChangeAction(a!, b!), `${a}/${b}`).toEqual({
        error: "Enter both 6-digit codes from your email.",
      });
    }
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it("caps attempts at ten an hour, on its own scope", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(confirmed());
    await confirmEmailChangeAction("111111", "222222");
    expect(sessionKey).toHaveBeenCalledWith(
      expect.anything(),
      "email:change-confirm",
    );
    expect(rateLimit).toHaveBeenCalledWith("k", {
      limit: 10,
      windowMs: 3_600_000,
    });
  });

  it("refuses a provider account even with two well-formed codes", async () => {
    vi.mocked(getServerSession).mockResolvedValue({
      userId: "user_1",
      email: "k@example.test",
      authMethod: "google",
    } as never);
    const res = await confirmEmailChangeAction("111111", "222222");
    expect(res.error).toMatch(/held by your identity provider/);
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  // ⚠ Auth revoked every session of the person with the change, this one
  // included, and refuses its bearer from then on. The console ends its half
  // at once — the blacklist makes the next navigation a sign-in — and neither
  // reseals the cookie nor revalidates: either would re-render, and the (app)
  // layout would redirect to sign-out before the person read the new address.
  it("ends this browser's session with the change, and re-renders nothing", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(confirmed());
    await confirmEmailChangeAction("111111", "222222");

    expect(blacklistSession).toHaveBeenCalledWith("sess_1");
    expect(writeSessionCookie).not.toHaveBeenCalled();
    expect(revalidatePath).not.toHaveBeenCalled();
  });

  it("ends nothing when nothing changed", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(null as never);
    expect(await confirmEmailChangeAction("111111", "222222")).toEqual({
      error: "The organization service is unreachable. Try again.",
    });

    vi.mocked(tryFetchWithTimeout).mockResolvedValue(
      new Response(null, { status: 400 }),
    );
    expect(await confirmEmailChangeAction("111111", "222222")).toEqual({
      error: "problem",
    });

    expect(blacklistSession).not.toHaveBeenCalled();
  });

  // Auth said the change happened, so the session is over either way; the
  // page names the address the codes were sent to instead of one we invent.
  it("still ends the session when auth will not say which address it landed on", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(
      new Response("not json", { status: 200 }),
    );

    expect(await confirmEmailChangeAction("111111", "222222")).toEqual({
      error: null,
    });
    expect(blacklistSession).toHaveBeenCalledWith("sess_1");
  });
});

describe("setDefaultOrganizationAction", () => {
  // The body names the organization; a header naming the one the console
  // stands in would be a second, different answer to the same question.
  it("asks auth to open the console in the organization chosen, named in the body alone", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(
      new Response(JSON.stringify({ defaultOrganizationId: "org_2" }), { status: 200 }),
    );

    expect(await setDefaultOrganizationAction("org_2")).toEqual({ error: null });

    const [url, init] = vi.mocked(tryFetchWithTimeout).mock.calls[0]!;
    expect(url).toBe("http://auth.test/internal/me/default-organization");
    expect(init).toEqual(
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ organizationId: "org_2" }),
      }),
    );
    expect(sentHeaders()).toEqual(
      expect.objectContaining({
        authorization: "Bearer at_1",
        "x-service-secret": "secret",
      }),
    );
    expect(sentHeaders()).not.toHaveProperty("x-organization-id");
  });

  it("hands back auth's refusal in its own words", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(new Response(null, { status: 403 }));
    expect(await setDefaultOrganizationAction("org_stranger")).toEqual({ error: "problem" });
  });

  it("says so when auth cannot be reached", async () => {
    vi.mocked(tryFetchWithTimeout).mockResolvedValue(null as never);
    expect(await setDefaultOrganizationAction("org_2")).toEqual({
      error: "The organization service is unreachable. Try again.",
    });
  });

  it("sends nothing for a session that has ended, or past the limit", async () => {
    vi.mocked(isSessionBlacklisted).mockResolvedValue(true);
    expect(await setDefaultOrganizationAction("org_2")).toEqual({
      error: "Your session expired — sign in again.",
    });

    vi.mocked(isSessionBlacklisted).mockResolvedValue(false);
    vi.mocked(rateLimit).mockResolvedValue({ retryAfterMs: 1_000 } as never);
    expect(await setDefaultOrganizationAction("org_2")).toEqual({
      error: "Too many requests — slow down a moment.",
    });
    expect(sessionKey).toHaveBeenCalledWith(
      expect.anything(),
      "account:default-organization",
    );

    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });
});
