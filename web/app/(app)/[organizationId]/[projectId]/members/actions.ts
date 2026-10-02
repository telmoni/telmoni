"use server";

import { revalidatePath } from "next/cache";
import { redirect } from "next/navigation";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import type { SessionData } from "@/lib/auth/session";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { Flag } from "@/lib/flags";
import { featureOff } from "@/lib/server/flags";
import {
  identityContext,
  projectHeaders,
} from "@/lib/server/entities/identity-context";
import { fetchProjects } from "@/lib/server/data";
import { projectMatches } from "@/lib/slug";
import { getServerSession } from "@/lib/server/session";
import {
  organizationChannel,
  publishEvent,
  publishToAll,
  userChannel,
} from "@/lib/events/publisher";
import { displayName } from "@/lib/user-display";
import { answerMemberAction, extractInviteLink } from "@/lib/members/shared";

export interface ActionResult {
  error: string | null;
}

export async function inviteMemberAction(
  projectId: string,
  email: string,
  role: "admin" | "member",
): Promise<ActionResult & { link?: string }> {
  const gate = await open("members:invite", 10, projectId);
  if ("error" in gate) return gate;
  const off = await featureOff(Flag.Members);
  if (off) return { error: off };

  const trimmed = email.trim();
  if (!trimmed) return { error: "Enter the email address of the person to invite." };

  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/projects/${encodeURIComponent(gate.projectId)}/invites`,
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
    const projects = await fetchProjects();
    const project = projects.find((p) => p.id === gate.projectId);
    await publishEvent(userChannel(trimmed), {
      type: "invite:created",
      data: {
        id: String(data.id),
        scope: "project",
        targetId: gate.projectId,
        targetName: project?.name || "Project",
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

export async function revokeInviteAction(
  projectId: string,
  inviteId: string,
): Promise<ActionResult> {
  const gate = await open("members:invite-revoke", 30, projectId);
  if ("error" in gate) return gate;
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/projects/${encodeURIComponent(gate.projectId)}/invites/${encodeURIComponent(inviteId)}`,
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

export async function updateMemberRoleAction(
  projectId: string,
  memberId: string,
  role: "admin" | "member",
): Promise<ActionResult> {
  const gate = await open("members:role", 30, projectId);
  if ("error" in gate) return gate;
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/projects/${encodeURIComponent(gate.projectId)}/members/${encodeURIComponent(memberId)}/role`,
    {
      method: "PUT",
      headers: { ...gate.headers, "content-type": "application/json" },
      body: JSON.stringify({ role }),
    },
  );
  return answer(res);
}

export async function removeMemberAction(
  projectId: string,
  memberId: string,
  memberEmail?: string,
): Promise<ActionResult> {
  const gate = await open("members:remove", 30, projectId);
  if ("error" in gate) return gate;
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/projects/${encodeURIComponent(gate.projectId)}/members/${encodeURIComponent(memberId)}`,
    { method: "DELETE", headers: gate.headers },
  );
  if (!res?.ok) return answer(res);

  if (memberEmail) {
    await publishEvent(userChannel(memberEmail), {
      type: "membership:removed",
      data: { organizationId: gate.organizationId, projectId: gate.projectId },
    });
  }
  return answer(res);
}

/// Offer the project to one of its admins. Nothing moves until they accept.
export async function offerProjectAction(
  projectId: string,
  memberId: string,
): Promise<ActionResult> {
  const gate = await open("members:transfer", 10, projectId);
  if ("error" in gate) return gate;
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/projects/${encodeURIComponent(gate.projectId)}/transfer`,
    {
      method: "POST",
      headers: { ...gate.headers, "content-type": "application/json" },
      body: JSON.stringify({ memberId }),
    },
  );
  if (!res?.ok) return answer(res);

  await tellOfferRecipients(res, gate.organizationId, gate.projectId);
  if (gate.organizationId) {
    revalidatePath(`/${gate.organizationId}/${gate.projectId}/members`);
  }
  revalidatePath(`/${gate.projectId}/members`);
  return { error: null };
}

/// Withdraw the project's live offer.
export async function cancelProjectOfferAction(projectId: string): Promise<ActionResult> {
  const gate = await open("members:transfer", 10, projectId);
  if ("error" in gate) return gate;
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/projects/${encodeURIComponent(gate.projectId)}/transfer`,
    { method: "DELETE", headers: gate.headers },
  );
  if (!res?.ok) return answer(res);

  await tellOfferRecipients(res, gate.organizationId, gate.projectId);
  if (gate.organizationId) {
    revalidatePath(`/${gate.organizationId}/${gate.projectId}/members`);
  }
  revalidatePath(`/${gate.projectId}/members`);
  return { error: null };
}

/// The live notice to everybody whose offer just changed, at the addresses
/// auth read off the roster — never ones the browser supplied. That is the
/// offer's holder, on a new offer also the admin whose live one it replaced,
/// and the organization the project is in, whose roster pages show the offer.
async function tellOfferRecipients(
  res: Response,
  organizationId: string,
  projectId: string,
): Promise<void> {
  const body = (await res.json().catch(() => null)) as {
    offeredToEmail?: unknown;
    withdrawnFromEmail?: unknown;
  } | null;
  const at = (email: unknown) => (typeof email === "string" ? userChannel(email) : null);
  await publishToAll(
    [at(body?.offeredToEmail), at(body?.withdrawnFromEmail), organizationChannel(organizationId)],
    { type: "ownership:changed", data: { organizationId, projectId } },
  );
}

export async function leaveProjectAction(
  projectId: string,
): Promise<ActionResult> {
  const gate = await open("members:leave", 20, projectId);
  if ("error" in gate) return gate;
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/memberships/${encodeURIComponent(projectId)}`,
    { method: "DELETE", headers: gate.headers },
  );
  return answer(res);
}

import {
  acceptIncomingInviteAction as _acceptIncomingInviteAction,
  declineIncomingInviteAction as _declineIncomingInviteAction,
} from "@/app/(app)/account/notifications/invite-actions";

// Forward incoming invite actions for backwards compatibility.
// These actions operate on personal invites and have been moved to account/notifications/invite-actions.
export async function acceptIncomingInviteAction(inviteId: string) {
  return _acceptIncomingInviteAction(inviteId);
}

export async function declineIncomingInviteAction(inviteId: string) {
  return _declineIncomingInviteAction(inviteId);
}

async function open(
  bucket: string,
  limit: number,
  projectId: string,
): Promise<
  | {
      base: string;
      projectId: string;
      /// The organization the session stands in, which holds the project:
      /// a project page renders only inside the active organization's listing.
      organizationId: string;
      headers: Record<string, string>;
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
  const ctx = await identityContext();
  if (!ctx) {
    return {
      error: "Couldn't resolve your organization right now. Try again in a moment.",
    };
  }
  const projects = await fetchProjects();
  const matched = projects.find((p) => projectMatches(p, projectId));
  const canonicalProjectId = matched?.id ?? projectId;

  return {
    base: env.SERVER_URL,
    projectId: canonicalProjectId,
    organizationId: ctx.organizationId,
    headers: projectHeaders(ctx, canonicalProjectId),
    session,
  };
}

async function answer(res: Response | null): Promise<ActionResult> {
  return answerMemberAction(res);
}
