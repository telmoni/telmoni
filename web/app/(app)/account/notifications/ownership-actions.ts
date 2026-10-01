"use server";

import { revalidatePath } from "next/cache";
import { redirect } from "next/navigation";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { organizationChannel, publishEvent, publishToAll } from "@/lib/events/publisher";
import { setActiveOrganizationCookie } from "@/lib/server/cookies";
import { personHeaders, sessionHeaders } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

// Answering an ownership offer. The organization is the OFFER's, named
// explicitly, not the one the console is standing in: an offer is answered
// from the notifications page, wherever the person happens to be. Auth checks
// that they hold a live offer there; this sends nothing it could trust.

async function answerOffer(
  organizationId: string,
  verb: "accept" | "decline",
): Promise<{ error: string | null }> {
  const session = await getServerSession();
  if (!session) redirect("/auth/login");

  // The id travels as a header; anything but an organization id is refused
  // here rather than making `fetch` throw on a stray character.
  if (!/^org_[A-Za-z0-9]+$/.test(organizationId)) {
    return { error: "That is not an organization." };
  }

  const limited = await rateLimit(sessionKey(session, "organization:ownership-answer"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/organization/owner-transfer/${verb}`,
    { method: "POST", headers: sessionHeaders(session, organizationId) },
  );
  if (!res) {
    return { error: "Organizations are unavailable right now — try again shortly." };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };

  // Everyone in the organization sees the roles move — the previous owner
  // included — and the offer leave the owner's roster.
  await publishEvent(organizationChannel(organizationId), {
    type: "ownership:changed",
    data: { organizationId },
  });
  revalidatePath("/", "layout");
  return { error: null };
}

export async function acceptOwnershipAction(
  organizationId: string,
): Promise<{ error: string | null }> {
  return answerOffer(organizationId, "accept");
}

export async function declineOwnershipAction(
  organizationId: string,
): Promise<{ error: string | null }> {
  return answerOffer(organizationId, "decline");
}

const PROJECT_ID = /^project_[A-Za-z0-9]+$/;
const ORGANIZATION_ID = /^org_[A-Za-z0-9]+$/;

/// What answering a project offer comes back with: on an accept, where to go
/// next — the project, now in the caller's organization.
export interface ProjectOfferAnswer {
  error: string | null;
  href?: string;
}

// Answering the offer of a project. The project is the OFFER's, named
// explicitly, and the lane is the person's: the offer is answered from the
// notifications page, wherever the console happens to be standing. Where the
// project lands is the caller's to say among the organizations they own, and
// auth's to check.
async function answerProjectOffer(
  projectId: string,
  verb: "accept" | "decline",
  organizationId?: string,
): Promise<ProjectOfferAnswer> {
  const session = await getServerSession();
  if (!session) redirect("/auth/login");

  // Both ids reach auth in the URL or the body; anything but the shape each
  // has is refused here rather than making `fetch` throw on a stray character.
  if (!PROJECT_ID.test(projectId)) return { error: "That is not a project." };
  if (organizationId !== undefined && !ORGANIZATION_ID.test(organizationId)) {
    return { error: "That is not an organization." };
  }

  const limited = await rateLimit(sessionKey(session, "project:offer-answer"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/projects/${encodeURIComponent(projectId)}/transfer/${verb}`,
    {
      method: "POST",
      headers: { ...personHeaders(session), "content-type": "application/json" },
      body: JSON.stringify(verb === "accept" && organizationId ? { organizationId } : {}),
    },
  );
  if (!res) {
    return { error: "Organizations are unavailable right now — try again shortly." };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };

  const body = (await res.json().catch(() => null)) as {
    organizationId?: unknown;
    previousOrganizationId?: unknown;
    ownerOrganizationId?: unknown;
  } | null;
  const id = (value: unknown) => (typeof value === "string" ? value : null);

  if (verb === "decline") {
    const owner = id(body?.ownerOrganizationId);
    if (owner) {
      await publishEvent(organizationChannel(owner), {
        type: "ownership:changed",
        data: { organizationId: owner, projectId },
      });
    }
    revalidatePath("/", "layout");
    return { error: null };
  }

  // Both organizations see the project move — the one it left, whose owner
  // is now an admin on it, and the one it landed in.
  const destination = id(body?.organizationId);
  const source = id(body?.previousOrganizationId);
  await publishToAll(
    [
      destination ? organizationChannel(destination) : null,
      source ? organizationChannel(source) : null,
    ],
    { type: "ownership:changed", data: { organizationId: destination ?? source ?? "", projectId } },
  );
  // ⚠ **The console has to FOLLOW the project, or the caller lands on a 404.**
  // It is in the destination now, and `[projectId]/layout.tsx` resolves the
  // listing for whatever organization the cookie names.
  if (destination) await setActiveOrganizationCookie(destination);
  revalidatePath("/", "layout");
  return { error: null, href: `/${projectId}` };
}

export async function acceptProjectOfferAction(
  projectId: string,
  organizationId?: string,
): Promise<ProjectOfferAnswer> {
  return answerProjectOffer(projectId, "accept", organizationId);
}

export async function declineProjectOfferAction(
  projectId: string,
): Promise<ProjectOfferAnswer> {
  return answerProjectOffer(projectId, "decline");
}
