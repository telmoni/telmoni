"use server";

import { cookies } from "next/headers";
import { revalidatePath } from "next/cache";
import { redirect } from "next/navigation";

import { type ActionResult, refused } from "@/lib/action-result";
import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { ACTIVE_ORGANIZATION_COOKIE } from "@/lib/proxy/organization";
import { type GivenRole, owns } from "@/lib/roles";
import { admitOrganization } from "@/lib/server/admit";
import { organizationHeaders } from "@/lib/server/entities/identity-context";
import {
  type Roster,
  answerMemberAction,
  inviteMember,
  offer,
  removeMember,
  revokeInvite,
  updateMemberRole,
  withdrawOffer,
} from "@/lib/server/roster";

// The organization's roster (`lib/server/roster.ts`): every action takes the
// organization the page rendered, by id, and acts on that one or on nothing.

const roster = (organizationId: string): Roster => ({ kind: "organization", organizationId });

export async function inviteOrganizationMemberAction(
  organizationId: string,
  email: string,
  role: GivenRole,
): Promise<ActionResult<{ link?: string }>> {
  return inviteMember(roster(organizationId), email, role);
}

export async function revokeOrganizationInviteAction(
  organizationId: string,
  inviteId: string,
): Promise<ActionResult> {
  return revokeInvite(roster(organizationId), inviteId);
}

export async function updateOrganizationMemberRoleAction(
  organizationId: string,
  memberId: string,
  role: GivenRole,
): Promise<ActionResult> {
  return updateMemberRole(roster(organizationId), memberId, role);
}

export async function removeOrganizationMemberAction(
  organizationId: string,
  memberId: string,
): Promise<ActionResult> {
  return removeMember(roster(organizationId), memberId);
}

/** The organization offered to one of its admins. Nothing moves until they accept. */
export async function offerOwnershipAction(
  organizationId: string,
  memberId: string,
): Promise<ActionResult> {
  return offer(roster(organizationId), memberId);
}

/** The organization's live offer withdrawn. */
export async function cancelOwnershipOfferAction(organizationId: string): Promise<ActionResult> {
  return withdrawOffer(roster(organizationId));
}

export async function leaveOrganizationAction(organizationId: string): Promise<ActionResult> {
  const gate = await admitOrganization({ call: "members:leave", limit: 20 }, organizationId);
  if (!gate.ok) return refused(gate.error);
  const { ctx } = gate;
  if (owns(ctx.role)) {
    return refused("You own this organization. To leave, hand it to an admin first or delete it.");
  }

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/organization/members/${encodeURIComponent(ctx.userId)}`,
    { method: "DELETE", headers: organizationHeaders(ctx) },
  );
  const ans = await answerMemberAction(res);
  if (!ans.ok) return ans;

  // Evict the active-organization cookie so `/console` falls back to their
  // first remaining organization or the no-organization gate.
  const jar = await cookies();
  jar.delete(ACTIVE_ORGANIZATION_COOKIE);
  revalidatePath("/", "layout");
  redirect("/console");
}
