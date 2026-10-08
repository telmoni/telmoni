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
// Mocked so "nobody is signed out" is an assertion rather than an absence:
// deleting an account ends the session, and this lane must not borrow that.
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
// The real `organizationHeaders`: what reaches auth is the assertion under test.
vi.mock("@/lib/server/entities/identity-context", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/identity-context")>()),
  identityContext: vi.fn(),
}));
// `/me` did not answer: the refusal for an unplaced caller is the generic one.
vi.mock("@/lib/server/entities/organization", () => ({
  getServerContext: vi.fn(async () => null),
}));
// The roster auth answers before the organization closes: whose open console
// is told, by the address auth holds for each.
vi.mock("@/lib/server/entities/organization-member", () => ({
  fetchOrganizationMembers: vi.fn(async () => ({
    kind: "ok",
    members: [
      { member_id: "user_1", email: "owner@example.test" },
      { member_id: "user_2", email: "admin@example.test" },
      { member_id: "user_3", email: "member@example.test" },
    ],
  })),
}));
const mockPublishEvent = vi.fn();
vi.mock("@/lib/events/publisher", () => ({
  publishEvent: (...args: unknown[]) => mockPublishEvent(...args),
  publishToAll: async (channels: (string | null | undefined)[], event: unknown) => {
    for (const channel of new Set(channels.filter(Boolean))) {
      await mockPublishEvent(channel, event);
    }
  },
  userChannel: (email: string) => `bfev:user:${email.toLowerCase()}`,
}));
const mockRedirect = vi.fn((to: string) => {
  throw new Error(`REDIRECT:${to}`);
});
vi.mock("next/navigation", () => ({
  redirect: (to: string) => mockRedirect(to),
}));
const mockRevalidate = vi.fn();
vi.mock("next/cache", () => ({
  revalidatePath: (...a: unknown[]) => mockRevalidate(...a),
}));
// Spied for the same reason as the revalidation: a cookie write also makes
// Next render the page again inside the action's own response.
const mockCookieWrite = vi.fn();
vi.mock("next/headers", () => ({
  cookies: async () => ({
    get: () => undefined,
    set: (...a: unknown[]) => mockCookieWrite("set", ...a),
    delete: (...a: unknown[]) => mockCookieWrite("delete", ...a),
  }),
}));

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { destroySession } from "@/lib/auth/session";
import { blacklistSession, isSessionBlacklisted } from "@/lib/auth/session-blacklist";
import { identityContext } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

import {
  deleteOrganizationAction,
  requestOrganizationDeletionCodeAction,
} from "./deletion-actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

const SESSION = {
  userId: "user_1",
  email: "owner@example.test",
  accessToken: "at_1",
  sessionRowId: "sess_1",
};
const OWNER_ONLY = "Only this organization's owner can delete it.";
const UNRESOLVED = "Couldn't resolve your organization right now. Try again in a moment.";
const UNREACHABLE = "The organization service is unreachable. Try again.";
const SWITCHED =
  "This page is out of date. Reload it to act on the organization you're viewing.";

// The organization the settings page rendered, and handed to the form.
const ORGANIZATION = "org_acme";

function standingAs(
  role: "owner" | "admin" | "member",
  organizationId: string = ORGANIZATION,
) {
  vi.mocked(identityContext).mockResolvedValue({
    userId: "user_1",
    organizationId,
    role,
    accessToken: "at_1",
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue(SESSION as never);
  vi.mocked(isSessionBlacklisted).mockResolvedValue(false);
  standingAs("owner");
});

describe("requestOrganizationDeletionCodeAction", () => {
  it("asks auth to mail the owner a code for the organization the page rendered", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 202 }));
    expect(await requestOrganizationDeletionCodeAction(ORGANIZATION)).toEqual({ ok: true });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/organization/deletion-code",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": ORGANIZATION,
          "x-service-secret": "secret",
        }),
      }),
    );
  });

  // ⚠ **THE BUG THIS PINS.** Alice owns A and B; this page shows A; another
  // tab switches the shared cookie to B. `/me` answers B at the click, so the
  // code was minted for B — and typing it deleted B while the page said A.
  // Whatever her role where she now stands, the answer is the switch: an
  // owner-only refusal would blame a role she holds in the organization she
  // is looking at.
  it.each(["owner", "admin", "member"] as const)(
    "refuses without asking auth when the request resolves another organization, standing as %s",
    async (role) => {
      standingAs(role, "org_elsewhere");
      expect(await requestOrganizationDeletionCodeAction(ORGANIZATION)).toEqual({
        error: SWITCHED,
      });
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  // Presentation's half of the rule — auth refuses anybody but the owner and
  // says so — but a code mailed to somebody who cannot spend it is a code
  // nobody should have been sent.
  it.each(["admin", "member"] as const)(
    "refuses an organization %s without asking auth",
    async (role) => {
      standingAs(role);
      expect(await requestOrganizationDeletionCodeAction(ORGANIZATION)).toEqual({
        error: OWNER_ONLY,
      });
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  it("refuses without asking auth when it cannot tell where the caller stands", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    expect(await requestOrganizationDeletionCodeAction(ORGANIZATION)).toEqual({
      error: UNRESOLVED,
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("reports an unreachable service and a refusal distinctly", async () => {
    fetchMock.mockResolvedValue(null);
    expect(await requestOrganizationDeletionCodeAction(ORGANIZATION)).toEqual({
      error: UNREACHABLE,
    });

    fetchMock.mockResolvedValue(new Response(null, { status: 403 }));
    expect(await requestOrganizationDeletionCodeAction(ORGANIZATION)).toEqual({
      error: "problem",
    });
  });
});

describe("deleteOrganizationAction", () => {
  it("rejects a code that is not six digits, asking auth nothing", async () => {
    for (const bad of ["abc", "12345", "1234567", "12345a", ""]) {
      expect(await deleteOrganizationAction(ORGANIZATION, bad), bad).toEqual({
        error: "Enter the 6-digit code from your email.",
      });
    }
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it.each(["owner", "admin", "member"] as const)(
    "refuses when the request resolves another organization, standing as %s, asking auth nothing",
    async (role) => {
      standingAs(role, "org_elsewhere");
      expect(await deleteOrganizationAction(ORGANIZATION, "123456")).toEqual({ error: SWITCHED });
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  it.each(["admin", "member"] as const)(
    "refuses an organization %s, asking auth nothing",
    async (role) => {
      standingAs(role);
      expect(await deleteOrganizationAction(ORGANIZATION, "123456")).toEqual({
        error: OWNER_ONLY,
      });
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  it("refuses without asking auth when it cannot tell where the caller stands", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    expect(await deleteOrganizationAction(ORGANIZATION, "123456")).toEqual({
      error: UNRESOLVED,
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // ⚠ **The organization goes; its owner does not.** They land in whatever
  // organization they still belong to, so the session must survive — the
  // account's own deletion is the one that signs out.
  it("deletes the organization the page rendered and leaves the owner signed in", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ purged: true }), { status: 202 }));
    expect(await deleteOrganizationAction(ORGANIZATION, "123456")).toEqual({
      ok: true,
      purged: true,
    });

    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/organization",
      expect.objectContaining({
        method: "DELETE",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": ORGANIZATION,
          "content-type": "application/json",
        }),
        body: JSON.stringify({ code: "123456" }),
      }),
    );
    expect(destroySession).not.toHaveBeenCalled();
    expect(blacklistSession).not.toHaveBeenCalled();
    expect(mockRedirect).not.toHaveBeenCalled();
  });

  // The organization closes for everybody at auth's answer. Each other
  // member's open console is told by their own channel, so a tab standing in
  // it leaves; the owner's is not, since theirs is showing the success screen.
  it("tells every other member on their own channel, and not the owner", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ purged: true }), { status: 202 }));
    expect(await deleteOrganizationAction(ORGANIZATION, "123456")).toEqual({
      ok: true,
      purged: true,
    });
    const gone = { type: "membership:removed", data: { organizationId: ORGANIZATION } };
    expect(mockPublishEvent).toHaveBeenCalledTimes(2);
    expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:admin@example.test", gone);
    expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:member@example.test", gone);
  });

  it("tells nobody when auth refuses the deletion", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 403 }));
    expect(await deleteOrganizationAction(ORGANIZATION, "123456")).toEqual({ error: "problem" });
    expect(mockPublishEvent).not.toHaveBeenCalled();
  });

  // ⚠ Revalidating the layout causes Next to re-render the page in the
  // action's response while the cookie still names the organization just
  // deleted, which would corrupt the success screen. Continue handles the
  // navigation explicitly.
  it("re-renders nothing once the organization is gone, so the success screen keeps its name", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ purged: true }), { status: 202 }));
    expect(await deleteOrganizationAction(ORGANIZATION, "123456")).toEqual({
      ok: true,
      purged: true,
    });
    expect(mockRevalidate).not.toHaveBeenCalled();
    expect(mockCookieWrite).not.toHaveBeenCalled();
  });

  // The organization is closed to everyone from auth's answer on, whether or
  // not the request's purge hook landed: the sweep retries it, and runs every
  // purge before its row goes. The deletion succeeds either way, and whether
  // the hook has landed is passed on, so the success screen does not announce
  // a cleanup still being retried.
  it("succeeds whether or not the request's purge landed, and says which", async () => {
    for (const purged of [true, false]) {
      fetchMock.mockResolvedValue(new Response(JSON.stringify({ purged }), { status: 202 }));
      expect(await deleteOrganizationAction(ORGANIZATION, "123456"), String(purged)).toEqual({
        ok: true,
        purged,
      });
    }
    expect(mockRevalidate).not.toHaveBeenCalled();
  });

  it("surfaces auth's refusal and leaves everything as it was", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 403 }));
    expect(await deleteOrganizationAction(ORGANIZATION, "123456")).toEqual({ error: "problem" });
    expect(mockRevalidate).not.toHaveBeenCalled();

    fetchMock.mockResolvedValue(null);
    expect(await deleteOrganizationAction(ORGANIZATION, "123456")).toEqual({
      error: UNREACHABLE,
    });
  });

  it("sends a session ended elsewhere to sign in, and deletes nothing", async () => {
    vi.mocked(isSessionBlacklisted).mockResolvedValue(true);
    await expect(deleteOrganizationAction(ORGANIZATION, "123456")).rejects.toThrow(
      "REDIRECT:/auth/login",
    );
    expect(isSessionBlacklisted).toHaveBeenCalledWith("sess_1");
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
