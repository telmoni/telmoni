"use server";

import { cookies } from "next/headers";
import { revalidatePath } from "next/cache";
import { redirect } from "next/navigation";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import type { SessionData } from "@/lib/auth/session";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { Flag } from "@/lib/flags";
import { featureOff } from "@/lib/server/flags";
import {
  organizationHeaders,
  identityContext,
} from "@/lib/server/entities/identity-context";
import { ACTIVE_ORGANIZATION_COOKIE } from "@/lib/proxy/organization";
import { activeOrganization, getServerContext } from "@/lib/server/entities/organization";
import { fetchOrganizationMembers } from "@/lib/server/entities/organization-member";
import { SWITCHED_ORGANIZATION, unplacedOrganization } from "@/lib/server/identity";
import { getServerSession } from "@/lib/server/session";
import {
  organizationChannel,
  publishEvent,
  publishToAll,
  userChannel,
} from "@/lib/events/publisher";
import { displayName } from "@/lib/user-display";
import { organizationLabel } from "@/lib/identity";
import { answerMemberAction, extractInviteLink } from "@/lib/members/shared";

export interface ActionResult {
  error: string | null;
}

export async function inviteOrganizationMemberAction(
  organizationId: string,
  email: string,
  role: "admin" | "member",
): Promise<ActionResult & { link?: string }> {
  const gate = await open("members:invite", 10, organizationId);
  if ("error" in gate) return gate;
  const off = await featureOff(Flag.Members);
  if (off) return { error: off };

  const trimmed = email.trim();
  if (!trimmed) return { error: "Enter the email address of the person to invite." };

  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/organization/invites`,
    {
      method: "POST",
      headers: { ...gate.headers, "content-type": "application/json" },
      body: JSON.stringify({ email: trimmed, role }),
    },
  );
  if (!res) {
    return { error: "Organizations are unavailable right now — try again shortly." };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };

  const body: unknown = await res.json().catch(() => null);
  const data = body as {
    id?: string;
    link?: string;
    expiresAt?: string;
    ownerOrganizationId?: string;
  } | null;
  // Guard against nullish link fields being converted to string "null".
  const link = extractInviteLink(data);

  if (data?.id && data.expiresAt) {
    await publishEvent(userChannel(trimmed), {
      type: "invite:created",
      data: {
        id: String(data.id),
        scope: "organization",
        targetId: gate.organizationId,
        // The ORGANIZATION's label, which `/me` would list for the same
        // invitation.
        targetName: gate.label,
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

  return { error: null, link };
}

export async function revokeOrganizationInviteAction(
  organizationId: string,
  inviteId: string,
): Promise<ActionResult> {
  const gate = await open("members:invite-revoke", 30, organizationId);
  if ("error" in gate) return gate;
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/organization/invites/${encodeURIComponent(inviteId)}`,
    { method: "DELETE", headers: gate.headers },
  );
  const ans = await answer(res);
  if (ans.error || !res) return ans;

  const body: unknown = await res.json().catch(() => null);
  const data = body as { email?: string; ownerOrganizationId?: string } | null;
  await publishToAll(
    [
      data?.email ? userChannel(data.email) : null,
      data?.ownerOrganizationId ? organizationChannel(data.ownerOrganizationId) : null,
    ],
    { type: "invite:revoked", data: { inviteId } },
  );
  return ans;
}

export async function updateOrganizationMemberRoleAction(
  organizationId: string,
  memberId: string,
  role: "admin" | "member",
): Promise<ActionResult> {
  const gate = await open("members:role", 30, organizationId);
  if ("error" in gate) return gate;
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/organization/members/${encodeURIComponent(memberId)}/role`,
    {
      method: "PUT",
      headers: { ...gate.headers, "content-type": "application/json" },
      body: JSON.stringify({ role }),
    },
  );
  return answer(res);
}

export async function removeOrganizationMemberAction(
  organizationId: string,
  memberId: string,
): Promise<ActionResult> {
  const gate = await open("members:remove", 30, organizationId);
  if ("error" in gate) return gate;
  // The channel told is the removed person's own console, so their address
  // comes from the roster auth answers, never from the caller, who could
  // otherwise name anybody's.
  const roster = await fetchOrganizationMembers();
  const removed =
    roster.kind === "ok" ? roster.members.find((m) => m.member_id === memberId) : undefined;
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/organization/members/${encodeURIComponent(memberId)}`,
    { method: "DELETE", headers: gate.headers },
  );
  if (!res?.ok) return answer(res);

  if (removed) {
    await publishEvent(userChannel(removed.email), {
      type: "membership:removed",
      data: { organizationId: gate.organizationId },
    });
  }
  return answer(res);
}

/// Offer the organization to one of its members. Nothing moves until they accept.
export async function offerOwnershipAction(
  organizationId: string,
  memberId: string,
): Promise<ActionResult> {
  const gate = await open("members:transfer", 10, organizationId, "ownership");
  if ("error" in gate) return gate;
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/organization/owner-transfer`,
    {
      method: "POST",
      headers: { ...gate.headers, "content-type": "application/json" },
      body: JSON.stringify({ memberId }),
    },
  );
  if (!res?.ok) return answer(res);

  await tellOfferRecipients(res, gate.organizationId);
  revalidatePath("/(app)/[organization]/~/members", "page");
  return { error: null };
}

/// Withdraw the organization's live offer.
export async function cancelOwnershipOfferAction(organizationId: string): Promise<ActionResult> {
  const gate = await open("members:transfer", 10, organizationId, "ownership");
  if ("error" in gate) return gate;
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/organization/owner-transfer`,
    { method: "DELETE", headers: gate.headers },
  );
  if (!res?.ok) return answer(res);

  await tellOfferRecipients(res, gate.organizationId);
  revalidatePath("/(app)/[organization]/~/members", "page");
  return { error: null };
}

/// The live notice to everybody whose offer just changed, at the addresses
/// auth read off the roster — never ones the browser supplied. That is the
/// offer's holder, on a new offer also the member whose live one it replaced,
/// and the organization itself.
async function tellOfferRecipients(res: Response, organizationId: string): Promise<void> {
  const body = (await res.json().catch(() => null)) as {
    offeredToEmail?: unknown;
    withdrawnFromEmail?: unknown;
  } | null;
  const at = (email: unknown) => (typeof email === "string" ? userChannel(email) : null);
  await publishToAll(
    [at(body?.offeredToEmail), at(body?.withdrawnFromEmail)],
    { type: "ownership:changed", data: { organizationId } },
  );
}

export async function leaveOrganizationAction(
  organizationId: string,
): Promise<ActionResult> {
  const session = await getServerSession();
  if (!session) redirect("/auth/login");
  const limited = await rateLimit(sessionKey(session, "members:leave"), {
    limit: 20,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const ctx = await identityContext();
  if (!ctx) return { error: await unplacedOrganization() };
  if (ctx.organizationId !== organizationId) return { error: SWITCHED_ORGANIZATION };
  if (ctx.role === "owner") {
    return {
      error: "You own this organization. To leave, hand it to an admin first or delete it.",
    };
  }

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/organization/members/${encodeURIComponent(ctx.userId)}`,
    { method: "DELETE", headers: organizationHeaders(ctx) },
  );
  if (!res) {
    return { error: "Organizations are unavailable right now — try again shortly." };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };

  // Evict the active-organization cookie so `/console` falls back to their
  // first remaining organization or the no-organization gate.
  const jar = await cookies();
  jar.delete(ACTIVE_ORGANIZATION_COOKIE);
  revalidatePath("/", "layout");
  redirect("/console");
}

/// Presentation's half of the rule, mirrored from auth, which enforces it on
/// its own rows: members, their roles and invitations are an owner's or an
/// admin's to manage (`can_manage_org_members`); the organization itself is
/// the owner's alone to hand over (`require_owner`). The page draws the
/// controls by the same rule, so a refusal here is a stale page, never a
/// button that cannot work.
async function open(
  bucket: string,
  limit: number,
  organizationId: string,
  lane: "members" | "ownership" = "members",
): Promise<
  | {
      base: string;
      organizationId: string;
      headers: Record<string, string>;
      label: string;
      session: SessionData;
    }
  | { error: string; base?: undefined }
> {
  const session = await getServerSession();
  if (!session) redirect("/auth/login");
  const limited = await rateLimit(sessionKey(session, bucket), {
    limit,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };
  const [ctx, context] = await Promise.all([
    identityContext(),
    getServerContext(),
  ]);
  const organization = context ? activeOrganization(context) : null;
  if (!ctx || !organization) return { error: await unplacedOrganization() };
  if (ctx.organizationId !== organizationId) return { error: SWITCHED_ORGANIZATION };
  if (lane === "ownership" && ctx.role !== "owner") {
    return { error: "Only the organization owner can hand it over." };
  }
  if (ctx.role !== "owner" && ctx.role !== "admin") {
    return { error: "Only an organization owner or admin can manage organization members." };
  }
  return {
    base: env.SERVER_URL,
    headers: organizationHeaders(ctx),
    organizationId: ctx.organizationId,
    label: organizationLabel(organization),
    session,
  };
}

async function answer(res: Response | null): Promise<ActionResult> {
  return answerMemberAction(res);
}
