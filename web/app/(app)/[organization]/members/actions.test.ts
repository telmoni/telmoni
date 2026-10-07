import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/entities/identity-context", () => ({
  identityContext: vi.fn(),
  organizationHeaders: vi.fn((ctx: { organizationId: string }) => ({
    authorization: "Bearer at_1",
    "x-organization-id": ctx.organizationId,
  })),
}));
// The real `activeOrganization`: the organization a notice names is read off it.
vi.mock("@/lib/server/entities/organization", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/organization")>()),
  getServerContext: vi.fn(),
}));
vi.mock("@/lib/server/flags", () => ({
  featureOff: vi.fn(async () => null),
}));
// The roster auth answers: where a removal reads the address it tells, since
// the caller's argument could name anybody's.
vi.mock("@/lib/server/entities/organization-member", () => ({
  fetchOrganizationMembers: vi.fn(async () => ({
    kind: "ok",
    members: [{ member_id: "user_2", email: "removed@example.test" }],
  })),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "problem" })),
}));
// `leaveOrganizationAction` is the only action here that redirects, clears a
// cookie and revalidates — the three things a LEAVE has to do that a removal
// performed by somebody else does not.
const mockRedirect = vi.fn((to: string) => {
  throw new Error(`REDIRECT:${to}`);
});
const mockCookieDelete = vi.fn();
const mockRevalidate = vi.fn();
vi.mock("next/navigation", () => ({
  redirect: (to: string) => mockRedirect(to),
}));
vi.mock("next/headers", () => ({
  cookies: async () => ({ delete: mockCookieDelete }),
}));
vi.mock("next/cache", () => ({
  revalidatePath: (...a: unknown[]) => mockRevalidate(...a),
}));
vi.mock("@/lib/env", () => ({
  env: {
    SERVER_URL: "http://auth.test",
    SERVICE_SECRET: "secret",
  },
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
  organizationChannel: (organizationId: string) => `bfev:organization:${organizationId}`,
}));

import { identityContext } from "@/lib/server/entities/identity-context";
import { getServerContext } from "@/lib/server/entities/organization";
import { getServerSession } from "@/lib/server/session";
import { tryFetchWithTimeout } from "@/lib/api/fetch";

import {
  cancelOwnershipOfferAction,
  inviteOrganizationMemberAction,
  leaveOrganizationAction,
  offerOwnershipAction,
  revokeOrganizationInviteAction,
  removeOrganizationMemberAction,
  updateOrganizationMemberRoleAction,
} from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

// The organization the members page rendered, and handed to every control.
const ORGANIZATION = "org_acme";
const MANAGERS_ONLY = "Only an organization owner or admin can manage organization members.";
const OWNER_ONLY = "Only the organization owner can hand it over.";
const UNRESOLVED = "Couldn't resolve your organization right now. Try again in a moment.";
const SWITCHED =
  "This page is out of date. Reload it to act on the organization you're viewing.";

// Auth's answer to an offer or a withdrawal: who holds it, and at what address
// as the roster has it.
const offered = () =>
  new Response(
    JSON.stringify({ offeredTo: "user_admin", offeredToEmail: "Admin@Example.test" }),
    { status: 200 },
  );

// Every action behind `openOrganization`, called the way the members page
// calls it: for the organization it rendered.
const ROSTER_ACTIONS = [
  ["inviteOrganizationMemberAction", () =>
    inviteOrganizationMemberAction(ORGANIZATION, "new@example.test", "member")],
  ["revokeOrganizationInviteAction", () => revokeOrganizationInviteAction(ORGANIZATION, "inv_123")],
  ["updateOrganizationMemberRoleAction", () =>
    updateOrganizationMemberRoleAction(ORGANIZATION, "user_2", "admin")],
  ["removeOrganizationMemberAction", () => removeOrganizationMemberAction(ORGANIZATION, "user_2")],
  ["offerOwnershipAction", () => offerOwnershipAction(ORGANIZATION, "user_2")],
  ["cancelOwnershipOfferAction", () => cancelOwnershipOfferAction(ORGANIZATION)],
] as const;

// The caller in `organizationId` as `role`. Whether they may manage its roster
// turns on that role alone — the organization's id is nobody's user id.
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
  vi.mocked(getServerContext).mockResolvedValue({
    person: {
      userId: "user_1",
      email: "user@example.test",
      displayName: null,
      analyticsOptIn: false,
    },
    organizations: [
      { organizationId, slug: "acme", name: "Acme", ownerEmail: "owner@example.test", role },
    ],
    deletedOrganizations: [],
    activeOrganizationId: organizationId,
    defaultOrganizationId: organizationId,
    organizationNotFound: false,
    memberships: [],
    incomingInvites: [],
    projectOffers: [],
    flags: {},
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_1",
    email: "user@example.test",
  } as never);
});

describe("organization members actions", () => {
  // The gate is presentation's half of auth's rule, and the members page
  // draws its controls by the same one: a member sees none and may do none.
  describe("when acting as an organization member", () => {
    beforeEach(() => {
      standingAs("member");
    });

    it.each(ROSTER_ACTIONS.slice(0, 4))("%s refuses, asking auth nothing", async (_, act) => {
      expect(await act()).toEqual({ error: MANAGERS_ONLY });
      expect(fetchMock).not.toHaveBeenCalled();
    });

    it.each(ROSTER_ACTIONS.slice(4))("%s refuses, asking auth nothing and telling nobody", async (_, act) => {
      expect(await act()).toEqual({ error: OWNER_ONLY });
      expect(fetchMock).not.toHaveBeenCalled();
      expect(mockPublishEvent).not.toHaveBeenCalled();
    });
  });

  // ⚠ An ADMIN manages the roster — auth's `can_manage_org_members`, and the
  // controls the page draws them — and may not hand the organization over,
  // which is the owner's alone. The gate once refused admins everything, from
  // a page that showed them every control.
  describe("when acting as an organization admin", () => {
    beforeEach(() => {
      standingAs("admin");
    });

    it.each(ROSTER_ACTIONS.slice(0, 4))("%s goes to auth, under the organization the page rendered", async (_, act) => {
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      expect(await act()).toEqual({ error: null });
      expect(fetchMock).toHaveBeenCalledTimes(1);
      expect(fetchMock.mock.calls[0]![1]).toEqual(
        expect.objectContaining({
          headers: expect.objectContaining({ "x-organization-id": ORGANIZATION }),
        }),
      );
    });

    it.each([{ link: null }, { link: 42 }, {}])(
      "offers no link at all for %o rather than stringifying it",
      async (body) => {
        fetchMock.mockResolvedValue(
          new Response(JSON.stringify(body), { status: 201 }),
        );
        const res = await inviteOrganizationMemberAction(ORGANIZATION, "colleague@example.test", "member");
        expect(res.link).toBeUndefined();
      },
    );

    it.each(ROSTER_ACTIONS.slice(4))("%s refuses, asking auth nothing and telling nobody", async (_, act) => {
      expect(await act()).toEqual({ error: OWNER_ONLY });
      expect(fetchMock).not.toHaveBeenCalled();
      expect(mockPublishEvent).not.toHaveBeenCalled();
    });
  });

  // Posted from a tab whose organization changed its URL, or let the person
  // go, after the page rendered: its path names an organization auth did not
  // answer with. A reload would answer "not found", so "try again" would be
  // the wrong advice.
  describe("when the path names an organization auth did not answer with", () => {
    beforeEach(() => {
      standingAs("owner");
      vi.mocked(identityContext).mockResolvedValue(null);
      vi.mocked(getServerContext).mockResolvedValue({
        organizationNotFound: true,
      } as Awaited<ReturnType<typeof getServerContext>>);
    });

    it.each(ROSTER_ACTIONS)("%s says the address is gone, asking auth nothing", async (_, act) => {
      const res = await act();
      expect(res.error).toMatch(/URL was changed, or you're no longer in it/);
      expect(fetchMock).not.toHaveBeenCalled();
      expect(mockPublishEvent).not.toHaveBeenCalled();
    });
  });

  // ⚠ **The page rendered one organization; the request now resolves
  // another.** Every action here resolves the active organization when it is
  // called — from a path whose slug a URL change has since moved — so without
  // this check an owner of two organizations would invite, remove or hand over
  // in the one auth fell back to, from a roster that shows the first. The
  // refusal is the answer whatever the caller's role there: a role refusal
  // would blame a role they hold in the one they are looking at.
  describe.each(["owner", "admin", "member"] as const)(
    "when the request resolves another organization, standing there as %s",
    (role) => {
      beforeEach(() => {
        standingAs(role, "org_elsewhere");
      });

      it.each(ROSTER_ACTIONS)("%s refuses, asking auth nothing and telling nobody", async (_, act) => {
        expect(await act()).toEqual({ error: SWITCHED });
        expect(fetchMock).not.toHaveBeenCalled();
        expect(mockPublishEvent).not.toHaveBeenCalled();
        expect(mockRevalidate).not.toHaveBeenCalled();
      });
    },
  );

  describe("when identity context is unavailable (fail-closed)", () => {
    beforeEach(() => {
      standingAs("owner");
    });

    it("rejects inviteOrganizationMemberAction", async () => {
      vi.mocked(identityContext).mockResolvedValue(null);
      const res = await inviteOrganizationMemberAction(ORGANIZATION, "new@example.test", "member");
      expect(res).toEqual({ error: UNRESOLVED });
      expect(fetchMock).not.toHaveBeenCalled();
    });

    // The organization a notice names comes off `/me`; without it there is
    // nothing true to put on the recipient's screen, so nothing is sent.
    it("rejects offerOwnershipAction when auth cannot say which organization it is", async () => {
      vi.mocked(getServerContext).mockResolvedValue(null);
      const res = await offerOwnershipAction(ORGANIZATION, "user_2");
      expect(res).toEqual({ error: UNRESOLVED });
      expect(fetchMock).not.toHaveBeenCalled();
    });
  });

  describe("when acting as the organization owner", () => {
    beforeEach(() => {
      standingAs("owner");
    });

    it("allows inviteOrganizationMemberAction and forwards to the server", async () => {
      fetchMock.mockResolvedValue(
        new Response(JSON.stringify({ link: "http://test/invite/abc" }), { status: 200 }),
      );
      const res = await inviteOrganizationMemberAction(
        ORGANIZATION,
        "colleague@example.test",
        "admin",
      );
      expect(res).toEqual({ error: null, link: "http://test/invite/abc" });
      expect(fetchMock).toHaveBeenCalledWith(
        "http://auth.test/internal/organization/invites",
        expect.objectContaining({
          method: "POST",
          headers: expect.objectContaining({ "x-organization-id": ORGANIZATION }),
          body: JSON.stringify({ email: "colleague@example.test", role: "admin" }),
        }),
      );
      expect(mockPublishEvent).not.toHaveBeenCalled();
    });

    // ⚠ The offer's `targetName` was the INVITER's own name, which is what an
    // organization was called while it was its owner. It is the organization's
    // label now — what `/me` lists for the same invitation on the next read.
    it("pushes the offer once auth has assigned it an id and an expiry", async () => {
      fetchMock.mockResolvedValue(
        new Response(
          JSON.stringify({
            id: "inv_org_1",
            link: "http://test/invite/abc",
            expiresAt: "2026-09-20T00:00:00Z",
            ownerOrganizationId: ORGANIZATION,
          }),
          { status: 201 },
        ),
      );
      await inviteOrganizationMemberAction(ORGANIZATION, "Colleague@Example.test", "member");
      expect(mockPublishEvent).toHaveBeenCalledTimes(2);
      expect(mockPublishEvent).toHaveBeenCalledWith(`bfev:organization:${ORGANIZATION}`, {
        type: "invite:sent",
        data: { inviteId: "inv_org_1" },
      });
      expect(mockPublishEvent).toHaveBeenCalledWith(
        "bfev:user:colleague@example.test",
        expect.objectContaining({
          type: "invite:created",
          data: expect.objectContaining({
            id: "inv_org_1",
            scope: "organization",
            targetId: ORGANIZATION,
            targetName: "Acme",
            role: "member",
            expiresAt: "2026-09-20T00:00:00Z",
          }),
        }),
      );
    });

    it("tells the organization even when the invitee's offer cannot be built", async () => {
      fetchMock.mockResolvedValue(
        new Response(
          JSON.stringify({ id: "inv_org_1", link: "http://test/invite/abc", ownerOrganizationId: ORGANIZATION }),
          { status: 201 },
        ),
      );
      await inviteOrganizationMemberAction(ORGANIZATION, "colleague@example.test", "member");
      expect(mockPublishEvent).toHaveBeenCalledTimes(1);
      expect(mockPublishEvent).toHaveBeenCalledWith(`bfev:organization:${ORGANIZATION}`, {
        type: "invite:sent",
        data: { inviteId: "inv_org_1" },
      });
    });

    it("revokeOrganizationInviteAction tells the recipient auth names, and the organization", async () => {
      fetchMock.mockResolvedValue(
        new Response(
          JSON.stringify({
            inviteId: "inv_org_1",
            email: "Colleague@Example.test",
            ownerOrganizationId: ORGANIZATION,
          }),
          { status: 200 },
        ),
      );
      const res = await revokeOrganizationInviteAction(ORGANIZATION, "inv_org_1");
      expect(res).toEqual({ error: null });
      expect(fetchMock).toHaveBeenCalledWith(
        "http://auth.test/internal/organization/invites/inv_org_1",
        expect.objectContaining({
          method: "DELETE",
          headers: expect.objectContaining({ "x-organization-id": ORGANIZATION }),
        }),
      );
      const revoked = { type: "invite:revoked", data: { inviteId: "inv_org_1" } };
      expect(mockPublishEvent).toHaveBeenCalledTimes(2);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:colleague@example.test", revoked);
      expect(mockPublishEvent).toHaveBeenCalledWith(`bfev:organization:${ORGANIZATION}`, revoked);
    });

    it("updateOrganizationMemberRoleAction changes the role in the organization the page rendered", async () => {
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      const res = await updateOrganizationMemberRoleAction(ORGANIZATION, "user_2", "admin");
      expect(res).toEqual({ error: null });
      expect(fetchMock).toHaveBeenCalledWith(
        "http://auth.test/internal/organization/members/user_2/role",
        expect.objectContaining({
          method: "PUT",
          headers: expect.objectContaining({
            "x-organization-id": ORGANIZATION,
            "content-type": "application/json",
          }),
          body: JSON.stringify({ role: "admin" }),
        }),
      );
    });

    it("tells the member whose role changed, at the roster's address, and nobody else", async () => {
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      const res = await updateOrganizationMemberRoleAction(ORGANIZATION, "user_2", "admin");
      expect(res).toEqual({ error: null });
      expect(mockPublishEvent).toHaveBeenCalledTimes(1);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:removed@example.test", {
        type: "membership:changed",
        data: { organizationId: ORGANIZATION },
      });
    });

    it("tells nobody about a role change auth refused", async () => {
      fetchMock.mockResolvedValue(new Response(null, { status: 403 }));
      const res = await updateOrganizationMemberRoleAction(ORGANIZATION, "user_2", "admin");
      expect(res).toEqual({ error: "problem" });
      expect(mockPublishEvent).not.toHaveBeenCalled();
    });

    it("removeOrganizationMemberAction removes from the organization the page rendered", async () => {
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      const res = await removeOrganizationMemberAction(ORGANIZATION, "user_2");
      expect(res).toEqual({ error: null });
      expect(fetchMock).toHaveBeenCalledWith(
        "http://auth.test/internal/organization/members/user_2",
        expect.objectContaining({
          method: "DELETE",
          headers: expect.objectContaining({ "x-organization-id": ORGANIZATION }),
        }),
      );
    });

    it("notifies the removed member on their personal channel, at the roster's address", async () => {
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      const res = await removeOrganizationMemberAction(ORGANIZATION, "user_2");
      expect(res).toEqual({ error: null });
      expect(mockPublishEvent).toHaveBeenCalledWith(
        "bfev:user:removed@example.test",
        {
          type: "membership:removed",
          data: { organizationId: ORGANIZATION },
        },
      );
    });
  });
});

// Nothing moves until the admin accepts. The notice goes to THEIR console, so
// an offer shows up — or goes away — without anybody reloading.
describe("offerOwnershipAction", () => {
  beforeEach(() => {
    standingAs("owner");
  });

  it("asks auth to offer the organization to one member, under the owner's headers", async () => {
    fetchMock.mockResolvedValue(offered());
    const res = await offerOwnershipAction(ORGANIZATION, "user_admin");
    expect(res).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/organization/owner-transfer",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": ORGANIZATION,
          "content-type": "application/json",
        }),
        body: JSON.stringify({ memberId: "user_admin" }),
      }),
    );
  });

  // ⚠ At the address AUTH read off the roster. The browser must not supply it,
  // preventing an owner from pushing a refresh onto an arbitrary address.
  it("tells the admin's console, at the address auth names, once the offer is made", async () => {
    fetchMock.mockResolvedValue(offered());
    await offerOwnershipAction(ORGANIZATION, "user_admin");
    expect(mockPublishEvent).toHaveBeenCalledTimes(1);
    expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:admin@example.test", {
      type: "ownership:changed",
      data: { organizationId: ORGANIZATION },
    });
    expect(mockRevalidate).toHaveBeenCalledWith("/(app)/[organization]/members", "page");
  });

  // ⚠ **One live offer per organization, so a new one withdraws the last.**
  // Both the new recipient and the replaced admin are informed at the addresses
  // auth names off the roster.
  it("also tells the admin whose live offer the new one replaced", async () => {
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({
          offeredTo: "user_admin",
          offeredToEmail: "Admin@Example.test",
          withdrawnFromEmail: "Former@Example.test",
        }),
        { status: 200 },
      ),
    );
    expect(await offerOwnershipAction(ORGANIZATION, "user_admin")).toEqual({ error: null });

    const changed = { type: "ownership:changed", data: { organizationId: ORGANIZATION } };
    expect(mockPublishEvent).toHaveBeenCalledTimes(2);
    expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:admin@example.test", changed);
    expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:former@example.test", changed);
  });

  it("tells only the new recipient when the offer replaced nobody's", async () => {
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({
          offeredTo: "user_admin",
          offeredToEmail: "Admin@Example.test",
          withdrawnFromEmail: null,
        }),
        { status: 200 },
      ),
    );
    await offerOwnershipAction(ORGANIZATION, "user_admin");
    expect(mockPublishEvent).toHaveBeenCalledTimes(1);
    expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:admin@example.test", {
      type: "ownership:changed",
      data: { organizationId: ORGANIZATION },
    });
  });

  it("still tells the replaced admin when auth names no address for the new one", async () => {
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({
          offeredTo: "user_admin",
          offeredToEmail: null,
          withdrawnFromEmail: "Former@Example.test",
        }),
        { status: 200 },
      ),
    );
    await offerOwnershipAction(ORGANIZATION, "user_admin");
    expect(mockPublishEvent).toHaveBeenCalledTimes(1);
    expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:former@example.test", {
      type: "ownership:changed",
      data: { organizationId: ORGANIZATION },
    });
  });

  it("still succeeds, telling nobody, when auth names no address", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ offeredTo: "user_admin", offeredToEmail: null }), {
        status: 200,
      }),
    );
    expect(await offerOwnershipAction(ORGANIZATION, "user_admin")).toEqual({ error: null });
    expect(mockPublishEvent).not.toHaveBeenCalled();
    expect(mockRevalidate).toHaveBeenCalledWith("/(app)/[organization]/members", "page");
  });

  it("tells nobody when auth refuses the offer", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 409 }));
    expect(await offerOwnershipAction(ORGANIZATION, "user_admin")).toEqual({
      error: "problem",
    });
    expect(mockPublishEvent).not.toHaveBeenCalled();
    expect(mockRevalidate).not.toHaveBeenCalled();
  });

  it("reports an unreachable service rather than claiming the offer went out", async () => {
    fetchMock.mockResolvedValue(null);
    expect(await offerOwnershipAction(ORGANIZATION, "user_admin")).toEqual({
      error: "Organizations are unavailable right now — try again shortly.",
    });
    expect(mockPublishEvent).not.toHaveBeenCalled();
  });
});

describe("cancelOwnershipOfferAction", () => {
  beforeEach(() => {
    standingAs("owner");
  });

  // One live offer per organization, so withdrawing names nobody: the
  // organization in the header is the whole address.
  it("asks auth to withdraw the organization's offer, naming no member", async () => {
    fetchMock.mockResolvedValue(offered());
    const res = await cancelOwnershipOfferAction(ORGANIZATION);
    expect(res).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/organization/owner-transfer",
      expect.objectContaining({
        method: "DELETE",
        headers: expect.objectContaining({ "x-organization-id": ORGANIZATION }),
      }),
    );
    expect(fetchMock.mock.calls[0]![1]).not.toHaveProperty("body");
  });

  it("tells the admin's console, at the address auth names, that the offer is gone", async () => {
    fetchMock.mockResolvedValue(offered());
    await cancelOwnershipOfferAction(ORGANIZATION);
    expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:admin@example.test", {
      type: "ownership:changed",
      data: { organizationId: ORGANIZATION },
    });
    expect(mockRevalidate).toHaveBeenCalledWith("/(app)/[organization]/members", "page");
  });

  it("tells nobody when auth refuses the withdrawal", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 404 }));
    expect(await cancelOwnershipOfferAction(ORGANIZATION)).toEqual({
      error: "problem",
    });
    expect(mockPublishEvent).not.toHaveBeenCalled();
    expect(mockRevalidate).not.toHaveBeenCalled();
  });
});

describe("leaveOrganizationAction", () => {
  // Standing in somebody ELSE's organization, as a member — the one the
  // overview page rendered.
  const LEAVING = "org_other";

  beforeEach(() => {
    vi.mocked(identityContext).mockResolvedValue({
      userId: "user_1",
      organizationId: LEAVING,
      role: "member",
      accessToken: "at_1",
    });
  });

  it("removes the caller from the organization they are standing in", async () => {
    fetchMock.mockResolvedValue({ ok: true, status: 204 } as never);
    await expect(leaveOrganizationAction(LEAVING)).rejects.toThrow("REDIRECT:/console");

    const [url, init] = fetchMock.mock.calls[0]!;
    expect(
      url,
      "a leave must name the CALLER, never the organization being left",
    ).toBe("http://auth.test/internal/organization/members/user_1");
    expect((init as { method: string }).method).toBe("DELETE");
    expect((init as { headers: Record<string, string> }).headers["x-organization-id"]).toBe(
      LEAVING,
    );
  });

  // ⚠ The page said "Leave <A>"; A has since changed its URL, its path names
  // no organization, and auth answered with B. Sent on, the caller would have
  // walked out of B — every project seat in it with them — from a button
  // that named A.
  it.each(["member", "admin", "owner"] as const)(
    "refuses, asking auth nothing, when the request resolves another organization (%s there)",
    async (role) => {
      vi.mocked(identityContext).mockResolvedValue({
        userId: "user_1",
        organizationId: "org_elsewhere",
        role,
        accessToken: "at_1",
      });
      expect(await leaveOrganizationAction(LEAVING)).toEqual({ error: SWITCHED });
      expect(fetchMock).not.toHaveBeenCalled();
      expect(mockCookieDelete).not.toHaveBeenCalled();
      expect(mockRevalidate).not.toHaveBeenCalled();
      expect(mockRedirect).not.toHaveBeenCalled();
    },
  );

  // ⚠ The cookie still names the organization just left. Auth ignores an
  // organization the person is no longer in, so it would stop resolving on the
  // next `/me` — but until then every fetcher on the render is pointed at an
  // organization the caller is no longer on.
  it("drops the active-organization cookie before it redirects", async () => {
    fetchMock.mockResolvedValue({ ok: true, status: 204 } as never);
    await expect(leaveOrganizationAction(LEAVING)).rejects.toThrow("REDIRECT:/console");
    expect(mockCookieDelete).toHaveBeenCalledWith("telmoni-organization");
    expect(mockRevalidate).toHaveBeenCalled();
  });

  // ⚠ The door is shut to the OWNER: an organization cannot be left with
  // nobody holding it, so it is handed to an admin first.
  it("refuses to let the owner leave, and asks auth nothing", async () => {
    vi.mocked(identityContext).mockResolvedValue({
      userId: "user_1",
      organizationId: LEAVING,
      role: "owner",
      accessToken: "at_1",
    });
    const r = await leaveOrganizationAction(LEAVING);
    expect(r.error).toMatch(/you own this organization/i);
    expect(r.error).toMatch(/hand it to an admin/i);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(mockCookieDelete).not.toHaveBeenCalled();
  });

  it("lets an admin leave, as anybody but the owner may", async () => {
    vi.mocked(identityContext).mockResolvedValue({
      userId: "user_1",
      organizationId: LEAVING,
      role: "admin",
      accessToken: "at_1",
    });
    fetchMock.mockResolvedValue({ ok: true, status: 204 } as never);
    await expect(leaveOrganizationAction(LEAVING)).rejects.toThrow("REDIRECT:/console");
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("keeps the cookie when auth refuses the leave", async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 403 } as never);
    const r = await leaveOrganizationAction(LEAVING);
    expect(r.error).toBe("problem");
    expect(
      mockCookieDelete,
      "the cookie was cleared for a leave that did not happen",
    ).not.toHaveBeenCalled();
    expect(mockRedirect).not.toHaveBeenCalled();
  });
});
