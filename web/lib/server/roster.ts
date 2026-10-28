import "server-only";

import { refresh, revalidatePath } from "next/cache";
import { z } from "zod";

import { type ActionResult, type Refused, refused } from "@/lib/action-result";
import { extractProblem, readBody, tryFetchWithTimeout } from "@/lib/api/fetch";
import type { SessionData } from "@/lib/auth/session";
import { env } from "@/lib/env";
import {
  organizationChannel,
  publishEvent,
  publishToAll,
  userChannel,
} from "@/lib/events/publisher";
import { Flag } from "@/lib/flags";
import { organizationLabel } from "@/lib/identity";
import { type GivenRole, NOT_A_GIVEN_ROLE, isGivenRole } from "@/lib/roles";
import { type Ceiling, admitOrganization, admitProject } from "@/lib/server/admit";
import { fetchProjects } from "@/lib/server/data";
import { organizationHeaders } from "@/lib/server/entities/identity-context";
import { fetchMembers } from "@/lib/server/entities/member";
import { activeOrganization, getServerContext } from "@/lib/server/entities/organization";
import { fetchOrganizationMembers } from "@/lib/server/entities/organization-member";
import { featureOff } from "@/lib/server/flags";
import { unplacedOrganization } from "@/lib/server/identity";
import { InviteScope } from "@/lib/types/enums";
import { displayName } from "@/lib/user-display";

// A roster is the organization's or a project's: the same seats, invitations
// and offer of the whole, kept through one lane each by the same rules. Each
// page's actions name the roster the page rendered and hand it here, so an
// action acts on that one or on nothing — not on whichever one its request
// resolves once the page has gone stale — and a refusal here is a stale
// page, never a button that cannot work.

/** The roster a page rendered, by id. */
export type Roster =
  | { kind: "organization"; organizationId: string }
  | { kind: "project"; projectId: string };

/** `path` under the roster's lane, at the server. */
export function laneUrl(roster: Roster, path: string): string {
  const lane =
    roster.kind === "organization"
      ? "/internal/v1/organization"
      : `/internal/v1/projects/${encodeURIComponent(roster.projectId)}`;
  return `${env.SERVER_URL}${lane}${path}`;
}

/** The roster's page, drawn again when its offer moves. */
function pageOf(roster: Roster): string {
  return roster.kind === "organization"
    ? "/(app)/[organization]/members"
    : "/(app)/[organization]/[project]/members";
}

const OFFER_PATH = { organization: "/owner-transfer", project: "/transfer" } as const;

// Each field read alone: one auth spells otherwise leaves the rest.
const text = z.string().optional().catch(undefined);

/** What auth answers an invitation sent: what the live notices need, and the
 *  link when it hands one back. */
const InviteSentSchema = z.object({
  id: text,
  link: text,
  expiresAt: text,
  ownerOrganizationId: text,
});

/** A roster's change: whose console to tell, and the organization's. */
export const RosterChangeSchema = z.object({ email: text, ownerOrganizationId: text });

/** An offer's change: who holds it now, and whose it replaced. */
const OfferChangeSchema = z.object({ offeredToEmail: text, withdrawnFromEmail: text });

/** The lane's answer as an action's: a refusal in the lane's words, an
 *  outage in ours. */
export async function answerMemberAction(res: Response | null): Promise<ActionResult> {
  if (!res) {
    return refused("Organizations are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);
  return { ok: true };
}

/** What the roster's gate hands an action. */
export type RosterGate = {
  ok: true;
  headers: Record<string, string>;
  session: SessionData;
  /** The organization the roster is in: the one named, or the one the
   *  project's page renders inside. */
  organizationId: string;
  /** What the roster's live notices are about. */
  about: { organizationId: string; projectId?: string };
  /** What an invitation names as its target, as `/me` lists it. */
  name: () => Promise<string>;
};

/** Admitted to the roster, holding the role the call needs, as auth enforces
 *  on its own rows: an organization's seats are its owner's or an admin's to
 *  manage, the organization the owner's alone to hand over (`hands`); a
 *  project's lane answers for its own. */
export async function admitRoster(
  roster: Roster,
  ceiling: Ceiling,
  hands = false,
): Promise<RosterGate | Refused> {
  if (roster.kind === "project") {
    const { projectId } = roster;
    const gate = await admitProject(ceiling, projectId);
    if (!gate.ok) return refused(gate.error);
    return {
      ok: true,
      headers: gate.headers,
      session: gate.session,
      organizationId: gate.organizationId,
      about: { organizationId: gate.organizationId, projectId },
      name: async () =>
        (await fetchProjects()).find((p) => p.id === projectId)?.name || "Project",
    };
  }
  const gate = await admitOrganization(
    ceiling,
    roster.organizationId,
    hands
      ? { role: "owner", refused: "Only the organization owner can hand it over." }
      : {
          role: "admin",
          refused: "Only an organization owner or admin can manage organization members.",
        },
  );
  if (!gate.ok) return refused(gate.error);
  const context = await getServerContext();
  const organization = context ? activeOrganization(context) : null;
  if (!organization) return refused(await unplacedOrganization());
  const label = organizationLabel(organization);
  return {
    ok: true,
    headers: organizationHeaders(gate.ctx),
    session: gate.session,
    organizationId: gate.ctx.organizationId,
    about: { organizationId: gate.ctx.organizationId },
    name: async () => label,
  };
}

/** `email` invited onto the roster as `role`: the link, when auth hands one
 *  back, with their console and the organization's told. */
export async function inviteMember(
  roster: Roster,
  email: string,
  role: GivenRole,
): Promise<ActionResult<{ link?: string }>> {
  if (!isGivenRole(role)) return refused(NOT_A_GIVEN_ROLE);
  const gate = await admitRoster(roster, { call: "members:invite", limit: 10 });
  if (!gate.ok) return gate;
  const off = await featureOff(Flag.Members);
  if (off) return refused(off);
  const trimmed = email.trim();
  if (!trimmed) return refused("Enter the email address of the person to invite.");

  const res = await tryFetchWithTimeout(laneUrl(roster, "/invites"), {
    method: "POST",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify({ email: trimmed, role }),
  });
  const ans = await answerMemberAction(res);
  if (!ans.ok || !res) return ans;

  const data = await readBody(res, InviteSentSchema);
  if (data?.id && data.expiresAt) {
    await publishEvent(userChannel(trimmed), {
      type: "invite:created",
      data: {
        id: String(data.id),
        scope: roster.kind === "organization" ? InviteScope.Organization : InviteScope.Project,
        targetId: roster.kind === "organization" ? roster.organizationId : roster.projectId,
        // The target's label, which `/me` would list for the same invitation.
        targetName: await gate.name(),
        role,
        inviterEmail: gate.session.email,
        inviterDisplayName: displayName(gate.session),
        expiresAt: data.expiresAt,
        createdAt: new Date().toISOString(),
      },
    });
  }
  if (data?.id && data.ownerOrganizationId) {
    await publishEvent(organizationChannel(data.ownerOrganizationId), {
      type: "invite:sent",
      data: { inviteId: String(data.id) },
    });
  }
  refresh();
  return { ok: true, link: data?.link };
}

/** The invitation withdrawn: its link stops working, and its recipient's
 *  console and the organization's are told. */
export async function revokeInvite(roster: Roster, inviteId: string): Promise<ActionResult> {
  const gate = await admitRoster(roster, { call: "members:invite-revoke", limit: 30 });
  if (!gate.ok) return gate;
  const res = await tryFetchWithTimeout(
    laneUrl(roster, `/invites/${encodeURIComponent(inviteId)}`),
    { method: "DELETE", headers: gate.headers },
  );
  const ans = await answerMemberAction(res);
  if (!ans.ok || !res) return ans;

  const data = await readBody(res, RosterChangeSchema);
  await publishToAll(
    [
      data?.email ? userChannel(data.email) : null,
      data?.ownerOrganizationId ? organizationChannel(data.ownerOrganizationId) : null,
    ],
    { type: "invite:revoked", data: { inviteId } },
  );
  refresh();
  return ans;
}

/** The seat's address as auth has it: where a change tells its holder, never
 *  the caller's argument, which could name anybody's. */
async function seated(roster: Roster, memberId: string): Promise<{ email: string } | undefined> {
  const listing =
    roster.kind === "organization"
      ? await fetchOrganizationMembers()
      : await fetchMembers(roster.projectId);
  const members: readonly { member_id: string; email: string }[] =
    listing.kind === "ok" ? listing.members : [];
  return members.find((m) => m.member_id === memberId);
}

/** The seat's role changed, and its holder told. */
export async function updateMemberRole(
  roster: Roster,
  memberId: string,
  role: GivenRole,
): Promise<ActionResult> {
  if (!isGivenRole(role)) return refused(NOT_A_GIVEN_ROLE);
  const gate = await admitRoster(roster, { call: "members:role", limit: 30 });
  if (!gate.ok) return gate;
  const changed = await seated(roster, memberId);
  const res = await tryFetchWithTimeout(
    laneUrl(roster, `/members/${encodeURIComponent(memberId)}/role`),
    {
      method: "PUT",
      headers: { ...gate.headers, "content-type": "application/json" },
      body: JSON.stringify({ role }),
    },
  );
  if (!res?.ok) return answerMemberAction(res);
  if (changed) {
    await publishEvent(userChannel(changed.email), {
      type: "membership:changed",
      data: gate.about,
    });
  }
  refresh();
  return answerMemberAction(res);
}

/** The seat taken away, and its holder told at the address read first. */
export async function removeMember(roster: Roster, memberId: string): Promise<ActionResult> {
  const gate = await admitRoster(roster, { call: "members:remove", limit: 30 });
  if (!gate.ok) return gate;
  const removed = await seated(roster, memberId);
  const res = await tryFetchWithTimeout(
    laneUrl(roster, `/members/${encodeURIComponent(memberId)}`),
    { method: "DELETE", headers: gate.headers },
  );
  if (!res?.ok) return answerMemberAction(res);
  if (removed) {
    await publishEvent(userChannel(removed.email), {
      type: "membership:removed",
      data: gate.about,
    });
  }
  refresh();
  return answerMemberAction(res);
}

/** The whole offered to the seat's holder, an admin: nothing moves until they
 *  accept. */
export async function offer(roster: Roster, memberId: string): Promise<ActionResult> {
  const gate = await admitRoster(roster, { call: "members:transfer", limit: 10 }, true);
  if (!gate.ok) return gate;
  const res = await tryFetchWithTimeout(laneUrl(roster, OFFER_PATH[roster.kind]), {
    method: "POST",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify({ memberId }),
  });
  if (!res?.ok) return answerMemberAction(res);
  await tellOfferRecipients(res, roster, gate);
  revalidatePath(pageOf(roster), "page");
  return { ok: true };
}

/** The live offer withdrawn. */
export async function withdrawOffer(roster: Roster): Promise<ActionResult> {
  const gate = await admitRoster(roster, { call: "members:transfer", limit: 10 }, true);
  if (!gate.ok) return gate;
  const res = await tryFetchWithTimeout(laneUrl(roster, OFFER_PATH[roster.kind]), {
    method: "DELETE",
    headers: gate.headers,
  });
  if (!res?.ok) return answerMemberAction(res);
  await tellOfferRecipients(res, roster, gate);
  revalidatePath(pageOf(roster), "page");
  return { ok: true };
}

/** The live notice to everybody whose offer just changed, at the addresses
 *  auth read off the roster, never ones the browser supplied: the offer's
 *  holder, on a new offer also the admin whose live one it replaced, and for
 *  a project the organization it is in, whose pages show the offer. */
async function tellOfferRecipients(res: Response, roster: Roster, gate: RosterGate): Promise<void> {
  const body = await readBody(res, OfferChangeSchema);
  const at = (email: string | undefined) => (email ? userChannel(email) : null);
  await publishToAll(
    [
      at(body?.offeredToEmail),
      at(body?.withdrawnFromEmail),
      roster.kind === "project" ? organizationChannel(gate.organizationId) : null,
    ],
    { type: "ownership:changed", data: gate.about },
  );
}
