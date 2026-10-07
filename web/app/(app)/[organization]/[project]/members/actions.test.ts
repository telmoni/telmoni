import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/cache", () => ({
  revalidatePath: vi.fn(),
}));
vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/entities/identity-context", () => ({
  identityContext: vi.fn(),
  personHeaders: vi.fn(() => ({ authorization: "Bearer at_1" })),
  projectHeaders: vi.fn(() => ({
    authorization: "Bearer at_1",
    "x-organization-id": "org_1",
    "x-project-id": "project_1",
  })),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "invitation already used" })),
}));
vi.mock("@/lib/env", () => ({
  env: {
    SERVER_URL: "http://auth.test",
  },
}));
vi.mock("@/lib/server/flags", () => ({
  featureOff: vi.fn(async () => null),
}));
vi.mock("@/lib/server/data", () => ({
  fetchProjects: vi.fn(async () => [
    { id: "project_1", slug: "payments", name: "Payments", role: "owner" },
  ]),
}));
// The roster auth answers for the project: where a removal reads the address
// it tells, since the caller's argument could name anybody's.
vi.mock("@/lib/server/entities/member", () => ({
  fetchMembers: vi.fn(async () => ({
    kind: "ok",
    members: [{ member_id: "user_2", email: "removed@example.test" }],
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
  organizationChannel: (organizationId: string) => `bfev:organization:${organizationId}`,
}));

import { revalidatePath } from "next/cache";
import { NextResponse } from "next/server";

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit } from "@/lib/api/rate-limit";
import { identityContext, personHeaders } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

import {
  acceptIncomingInviteAction,
  cancelProjectOfferAction,
  declineIncomingInviteAction,
  inviteMemberAction,
  offerProjectAction,
  revokeInviteAction,
  removeMemberAction,
  updateMemberRoleAction,
} from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

describe("Incoming invites server actions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({
      userId: "usr_1",
      email: "user@example.test",
      accessToken: "at_1",
    } as never);
    vi.mocked(rateLimit).mockResolvedValue(null);
    vi.mocked(identityContext).mockResolvedValue({
      userId: "usr_1",
      organizationId: "org_1",
      role: "member",
      accessToken: "at_1",
    });
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ status: "accepted" }), { status: 200 }),
    );
  });

  describe("acceptIncomingInviteAction", () => {
    it("calls accept endpoint at the server url and returns ok", async () => {
      const res = await acceptIncomingInviteAction("inv_123");
      expect(res).toEqual({ error: null });
      expect(fetchMock).toHaveBeenCalledWith(
        "http://auth.test/internal/me/invites/inv_123/accept",
        expect.objectContaining({
          method: "POST",
          headers: expect.objectContaining({ authorization: "Bearer at_1" }),
        }),
      );
    });

    it("publishes invite:resolved to invitee, inviter, and owner organization channels", async () => {
      fetchMock.mockResolvedValue(
        new Response(
          JSON.stringify({
            status: "accepted",
            scope: "project",
            targetId: "project_1",
            inviteId: "inv_123",
            inviterEmail: "inviter@example.com",
            ownerOrganizationId: "org_owner",
          }),
          { status: 200 },
        ),
      );
      const res = await acceptIncomingInviteAction("inv_123");
      expect(res).toEqual({ error: null });
      expect(mockPublishEvent).toHaveBeenCalledWith(
        "bfev:user:user@example.test",
        { type: "invite:resolved", data: { inviteId: "inv_123" } },
      );
      expect(mockPublishEvent).toHaveBeenCalledWith(
        "bfev:user:inviter@example.com",
        { type: "invite:resolved", data: { inviteId: "inv_123" } },
      );
      expect(mockPublishEvent).toHaveBeenCalledWith(
        "bfev:organization:org_owner",
        { type: "invite:resolved", data: { inviteId: "inv_123" } },
      );
      expect(vi.mocked(revalidatePath)).toHaveBeenCalledWith(
        "/(app)/[organization]/projects",
        "page",
      );
    });

    // ⚠ Auth answers a null `inviterEmail` once the inviter has deleted their
    // account. The null failed the whole parse, and the organization's notice
    // was thrown away with the inviter's: its roster pages never heard.
    it("still tells the invitee and the organization when the inviter is gone", async () => {
      fetchMock.mockResolvedValue(
        new Response(
          JSON.stringify({
            status: "accepted",
            scope: "project",
            targetId: "project_1",
            inviteId: "inv_123",
            inviterEmail: null,
            ownerOrganizationId: "org_owner",
          }),
          { status: 200 },
        ),
      );
      expect(await acceptIncomingInviteAction("inv_123")).toEqual({ error: null });
      const settled = { type: "invite:resolved", data: { inviteId: "inv_123" } };
      expect(mockPublishEvent).toHaveBeenCalledTimes(2);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:user@example.test", settled);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:organization:org_owner", settled);
    });

    it("returns error message when rate limited", async () => {
      vi.mocked(rateLimit).mockResolvedValue(new NextResponse(null, { status: 429 }));
      const res = await acceptIncomingInviteAction("inv_123");
      expect(res.error).toMatch(/too many requests/i);
      expect(fetchMock).not.toHaveBeenCalled();
    });

    // ⚠ **The way in for somebody in no organization.** With sign-ups closed
    // an invitee stands in none. It goes out under the person's bearer alone.
    it("accepts for somebody in no organization, under their own bearer", async () => {
      vi.mocked(identityContext).mockResolvedValue(null);
      expect(await acceptIncomingInviteAction("inv_123")).toEqual({ error: null });
      expect(personHeaders).toHaveBeenCalledWith(
        expect.objectContaining({ accessToken: "at_1" }),
      );
      expect(identityContext).not.toHaveBeenCalled();
    });

    it("returns problem message when upstream fails", async () => {
      fetchMock.mockResolvedValue(
        new Response(JSON.stringify({ error: "conflict" }), { status: 409 }),
      );
      const res = await acceptIncomingInviteAction("inv_123");
      expect(res.error).toBe("invitation already used");
    });
  });

  describe("declineIncomingInviteAction", () => {
    it("calls decline endpoint at the server url and returns ok", async () => {
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      const res = await declineIncomingInviteAction("inv_456");
      expect(res).toEqual({ error: null });
      expect(fetchMock).toHaveBeenCalledWith(
        "http://auth.test/internal/me/invites/inv_456/decline",
        expect.objectContaining({
          method: "POST",
          headers: expect.objectContaining({ authorization: "Bearer at_1" }),
        }),
      );
    });

    // An invitation answers the PERSON: somebody in no organization — sign-ups
    // closed, their account screen showing — declines it all the same.
    it("declines for somebody in no organization, under their own bearer", async () => {
      vi.mocked(identityContext).mockResolvedValue(null);
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      expect(await declineIncomingInviteAction("inv_456")).toEqual({ error: null });
      expect(personHeaders).toHaveBeenCalledWith(
        expect.objectContaining({ accessToken: "at_1" }),
      );
      expect(identityContext).not.toHaveBeenCalled();
    });

    it("settles the offer for the inviter and the project, not only the invitee", async () => {
      fetchMock.mockResolvedValue(
        new Response(
          JSON.stringify({
            status: "declined",
            scope: "project",
            inviteId: "inv_456",
            inviterEmail: "Inviter@Example.com",
            ownerOrganizationId: "org_owner",
          }),
          { status: 200 },
        ),
      );
      const res = await declineIncomingInviteAction("inv_456");
      expect(res).toEqual({ error: null });
      const settled = { type: "invite:resolved", data: { inviteId: "inv_456" } };
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:user@example.test", settled);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:inviter@example.com", settled);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:organization:org_owner", settled);
      expect(vi.mocked(revalidatePath)).not.toHaveBeenCalled();
    });

    it("still settles the offer for the organization when the inviter is gone", async () => {
      fetchMock.mockResolvedValue(
        new Response(
          JSON.stringify({
            status: "declined",
            scope: "project",
            inviteId: "inv_456",
            inviterEmail: null,
            ownerOrganizationId: "org_owner",
          }),
          { status: 200 },
        ),
      );
      expect(await declineIncomingInviteAction("inv_456")).toEqual({ error: null });
      const settled = { type: "invite:resolved", data: { inviteId: "inv_456" } };
      expect(mockPublishEvent).toHaveBeenCalledTimes(2);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:user@example.test", settled);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:organization:org_owner", settled);
    });

    it("tells only the invitee when auth answers no body", async () => {
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      await declineIncomingInviteAction("inv_456");
      expect(mockPublishEvent).toHaveBeenCalledTimes(1);
      expect(mockPublishEvent).toHaveBeenCalledWith(
        "bfev:user:user@example.test",
        { type: "invite:resolved", data: { inviteId: "inv_456" } },
      );
    });
  });

  describe("revokeInviteAction", () => {
    it("tells the recipient auth names, and the project, that the offer is gone", async () => {
      fetchMock.mockResolvedValue(
        new Response(
          JSON.stringify({
            inviteId: "inv_789",
            email: "Invitee@Example.com",
            ownerOrganizationId: "org_owner",
          }),
          { status: 200 },
        ),
      );
      const res = await revokeInviteAction("project_1", "inv_789");
      expect(res).toEqual({ error: null });
      expect(fetchMock).toHaveBeenCalledWith(
        "http://auth.test/internal/projects/project_1/invites/inv_789",
        expect.objectContaining({ method: "DELETE" }),
      );
      const revoked = { type: "invite:revoked", data: { inviteId: "inv_789" } };
      expect(mockPublishEvent).toHaveBeenCalledTimes(2);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:invitee@example.com", revoked);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:organization:org_owner", revoked);
    });

    it("pushes nothing when auth names nobody", async () => {
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      const res = await revokeInviteAction("project_1", "inv_789");
      expect(res).toEqual({ error: null });
      expect(mockPublishEvent).not.toHaveBeenCalled();
    });

    it("pushes nothing when auth refuses", async () => {
      fetchMock.mockResolvedValue(
        new Response(JSON.stringify({ error: "nope" }), { status: 404 }),
      );
      const res = await revokeInviteAction("project_1", "inv_789");
      expect(res.error).toBe("invitation already used");
      expect(mockPublishEvent).not.toHaveBeenCalled();
    });
  });

  describe("inviteMemberAction", () => {
    it("pushes the offer with the id and expiry auth assigned", async () => {
      fetchMock.mockResolvedValue(
        new Response(
          JSON.stringify({
            id: "inv_9",
            link: "http://test/invite/abc",
            expiresAt: "2026-09-20T00:00:00Z",
            ownerOrganizationId: "org_owner",
          }),
          { status: 201 },
        ),
      );
      const res = await inviteMemberAction("project_1", " New@Example.test ", "admin");
      expect(res).toEqual({ error: null, link: "http://test/invite/abc" });
      expect(mockPublishEvent).toHaveBeenCalledTimes(2);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:organization:org_owner", {
        type: "invite:sent",
        data: { inviteId: "inv_9" },
      });
      expect(mockPublishEvent).toHaveBeenCalledWith(
        "bfev:user:new@example.test",
        expect.objectContaining({
          type: "invite:created",
          data: expect.objectContaining({
            id: "inv_9",
            scope: "project",
            targetId: "project_1",
            targetName: "Payments",
            role: "admin",
            expiresAt: "2026-09-20T00:00:00Z",
          }),
        }),
      );
    });

    it("tells the organization even when the invitee's offer cannot be built", async () => {
      fetchMock.mockResolvedValue(
        new Response(
          JSON.stringify({ id: "inv_9", link: "http://test/invite/abc", ownerOrganizationId: "org_owner" }),
          { status: 201 },
        ),
      );
      await inviteMemberAction("project_1", "new@example.test", "member");
      expect(mockPublishEvent).toHaveBeenCalledTimes(1);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:organization:org_owner", {
        type: "invite:sent",
        data: { inviteId: "inv_9" },
      });
    });

    it("pushes nothing rather than an offer with a made-up id", async () => {
      fetchMock.mockResolvedValue(
        new Response(JSON.stringify({ link: "http://test/invite/abc" }), { status: 201 }),
      );
      const res = await inviteMemberAction("project_1", "new@example.test", "member");
      expect(res).toEqual({ error: null, link: "http://test/invite/abc" });
      expect(mockPublishEvent).not.toHaveBeenCalled();
    });

    // The test above pins the present-and-a-string case. This is the one the
    // `"link" in data` check let through: a null field is "in" the object, and
    // `String(null)` is the string "null" — offered to the inviter as the
    // link to send.
    it.each([{ link: null }, { link: 42 }, {}])(
      "offers no link at all for %o rather than stringifying it",
      async (body) => {
        fetchMock.mockResolvedValue(
          new Response(JSON.stringify(body), { status: 201 }),
        );
        const res = await inviteMemberAction("project_1", "new@example.test", "member");
        expect(res.link).toBeUndefined();
      },
    );
  });

  // The offer and its withdrawal go to the project's transfer lane, and the
  // live notice to the addresses auth read off the roster — the holder, the
  // admin whose offer a new one replaced — and to the organization whose
  // roster pages show it, never to an address the browser supplied.
  describe("handing the project over", () => {
    const changed = {
      type: "ownership:changed",
      data: { organizationId: "org_1", projectId: "project_1" },
    };

    it("offers the project and tells the recipient, the replaced holder and the organization", async () => {
      fetchMock.mockResolvedValue(
        new Response(
          JSON.stringify({
            offeredTo: "user_2",
            offeredToEmail: "Admin@Example.com",
            withdrawnFromEmail: "prior@example.com",
            expiresAt: "2026-10-05T00:00:00Z",
            ownerOrganizationId: "org_1",
          }),
          { status: 200 },
        ),
      );
      expect(await offerProjectAction("project_1", "user_2")).toEqual({ error: null });
      expect(fetchMock).toHaveBeenCalledWith(
        "http://auth.test/internal/projects/project_1/transfer",
        expect.objectContaining({
          method: "POST",
          body: JSON.stringify({ memberId: "user_2" }),
        }),
      );
      expect(mockPublishEvent).toHaveBeenCalledTimes(3);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:admin@example.com", changed);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:prior@example.com", changed);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:organization:org_1", changed);
      expect(vi.mocked(revalidatePath)).toHaveBeenCalledWith(
        "/(app)/[organization]/[project]/members",
        "page",
      );
    });

    it("withdraws the offer and tells whoever held it", async () => {
      fetchMock.mockResolvedValue(
        new Response(
          JSON.stringify({
            offeredTo: "user_2",
            offeredToEmail: "admin@example.com",
            ownerOrganizationId: "org_1",
          }),
          { status: 200 },
        ),
      );
      expect(await cancelProjectOfferAction("project_1")).toEqual({ error: null });
      expect(fetchMock).toHaveBeenCalledWith(
        "http://auth.test/internal/projects/project_1/transfer",
        expect.objectContaining({ method: "DELETE" }),
      );
      expect(mockPublishEvent).toHaveBeenCalledTimes(2);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:admin@example.com", changed);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:organization:org_1", changed);
    });

    it("tells nobody when auth refuses", async () => {
      fetchMock.mockResolvedValue(
        new Response(JSON.stringify({ error: "nope" }), { status: 409 }),
      );
      expect((await offerProjectAction("project_1", "user_2")).error).toBe(
        "invitation already used",
      );
      expect((await cancelProjectOfferAction("project_1")).error).toBe(
        "invitation already used",
      );
      expect(mockPublishEvent).not.toHaveBeenCalled();
      expect(vi.mocked(revalidatePath)).not.toHaveBeenCalled();
    });
  });
  describe("removeMemberAction", () => {
    it("tells nobody about a removal the roster does not know", async () => {
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      const res = await removeMemberAction("project_1", "user_9");
      expect(res).toEqual({ error: null });
      expect(mockPublishEvent).not.toHaveBeenCalled();
    });

    it("calls delete member endpoint and tells the removed member at the roster's address", async () => {
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      const res = await removeMemberAction("project_1", "user_2");
      expect(res).toEqual({ error: null });
      expect(fetchMock).toHaveBeenCalledWith(
        "http://auth.test/internal/projects/project_1/members/user_2",
        expect.objectContaining({
          method: "DELETE",
        }),
      );
      expect(mockPublishEvent).toHaveBeenCalledWith(
        "bfev:user:removed@example.test",
        {
          type: "membership:removed",
          data: { organizationId: "org_1", projectId: "project_1" },
        },
      );
    });
  });

  describe("updateMemberRoleAction", () => {
    it("changes the role and tells the member at the roster's address, and nobody else", async () => {
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      const res = await updateMemberRoleAction("project_1", "user_2", "admin");
      expect(res).toEqual({ error: null });
      expect(fetchMock).toHaveBeenCalledWith(
        "http://auth.test/internal/projects/project_1/members/user_2/role",
        expect.objectContaining({ method: "PUT", body: JSON.stringify({ role: "admin" }) }),
      );
      expect(mockPublishEvent).toHaveBeenCalledTimes(1);
      expect(mockPublishEvent).toHaveBeenCalledWith("bfev:user:removed@example.test", {
        type: "membership:changed",
        data: { organizationId: "org_1", projectId: "project_1" },
      });
    });

    it("tells nobody about a role change the roster does not know, or auth refused", async () => {
      fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
      expect(await updateMemberRoleAction("project_1", "user_9", "admin")).toEqual({ error: null });
      fetchMock.mockResolvedValue(new Response(null, { status: 403 }));
      expect(await updateMemberRoleAction("project_1", "user_2", "admin")).toEqual({
        error: "invitation already used",
      });
      expect(mockPublishEvent).not.toHaveBeenCalled();
    });
  });
});
