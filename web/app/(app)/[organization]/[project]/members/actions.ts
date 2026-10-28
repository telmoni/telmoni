"use server";

import { refresh, revalidatePath } from "next/cache";
import { redirect } from "next/navigation";

import { type ActionResult, refused } from "@/lib/action-result";
import { readBody, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { publishEvent, userChannel } from "@/lib/events/publisher";
import { Flag } from "@/lib/flags";
import { type GivenRole, NOT_A_GIVEN_ROLE, isGivenRole } from "@/lib/roles";
import { activeOrganization, getServerContext } from "@/lib/server/data";
import { featureOff } from "@/lib/server/flags";
import {
  type Roster,
  RosterChangeSchema,
  admitRoster,
  answerMemberAction,
  inviteMember,
  laneUrl,
  offer,
  removeMember,
  revokeInvite,
  updateMemberRole,
  withdrawOffer,
} from "@/lib/server/roster";
import { organizationPath } from "@/lib/slug";

// The project's roster (`lib/server/roster.ts`): every action takes the
// project its page rendered, by id, and acts on that one or on nothing.

const roster = (projectId: string): Roster => ({ kind: "project", projectId });

export async function inviteMemberAction(
  projectId: string,
  email: string,
  role: GivenRole,
): Promise<ActionResult<{ link?: string }>> {
  return inviteMember(roster(projectId), email, role);
}

/** A seat for a member of the organization, given with nothing to accept: the
 *  lane refuses anyone off its roster, and anyone but the organization's owner
 *  and admins. Their console learns of the project at the address auth
 *  answered, never one the browser supplied. */
export async function addMemberAction(
  projectId: string,
  memberId: string,
  role: GivenRole,
): Promise<ActionResult> {
  if (!isGivenRole(role)) return refused(NOT_A_GIVEN_ROLE);
  const of = roster(projectId);
  const gate = await admitRoster(of, { call: "members:add", limit: 30 });
  if (!gate.ok) return gate;
  const off = await featureOff(Flag.Members);
  if (off) return refused(off);

  const res = await tryFetchWithTimeout(laneUrl(of, "/members"), {
    method: "POST",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify({ memberId, role }),
  });
  if (!res?.ok) {
    // Refused off a roster drawn before somebody seated or removed the person:
    // the page is drawn again with what stands.
    if (res?.status === 404 || res?.status === 409) refresh();
    return answerMemberAction(res);
  }

  const email = (await readBody(res, RosterChangeSchema))?.email;
  if (email) {
    await publishEvent(userChannel(email), { type: "membership:changed", data: gate.about });
  }
  refresh();
  return { ok: true };
}

export async function revokeInviteAction(
  projectId: string,
  inviteId: string,
): Promise<ActionResult> {
  return revokeInvite(roster(projectId), inviteId);
}

export async function updateMemberRoleAction(
  projectId: string,
  memberId: string,
  role: GivenRole,
): Promise<ActionResult> {
  return updateMemberRole(roster(projectId), memberId, role);
}

export async function removeMemberAction(
  projectId: string,
  memberId: string,
): Promise<ActionResult> {
  return removeMember(roster(projectId), memberId);
}

/** The project offered to one of its admins. Nothing moves until they accept. */
export async function offerProjectAction(
  projectId: string,
  memberId: string,
): Promise<ActionResult> {
  return offer(roster(projectId), memberId);
}

/** The project's live offer withdrawn. */
export async function cancelProjectOfferAction(projectId: string): Promise<ActionResult> {
  return withdrawOffer(roster(projectId));
}

/** The caller's own seat given up. From the project's own Members page the
 *  seat was all that let them in, so `from: "project"` leads them to the
 *  organization. */
export async function leaveProjectAction(
  projectId: string,
  from: "organization" | "project" = "organization",
): Promise<ActionResult> {
  const gate = await admitRoster(roster(projectId), { call: "members:leave", limit: 20 });
  if (!gate.ok) return gate;
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/memberships/${encodeURIComponent(projectId)}`,
    { method: "DELETE", headers: gate.headers },
  );
  if (!res?.ok) return answerMemberAction(res);
  if (from === "project") {
    const ctx = await getServerContext();
    const home = ctx ? activeOrganization(ctx) : null;
    revalidatePath("/", "layout");
    redirect(home ? organizationPath(home.slug) : "/console");
  }
  refresh();
  return answerMemberAction(res);
}
