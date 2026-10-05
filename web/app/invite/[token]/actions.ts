"use server";

import { revalidatePath } from "next/cache";
import { redirect } from "next/navigation";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { setActiveOrganizationCookie } from "@/lib/server/cookies";
import { personHeaders } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";
import {
  organizationChannel,
  publishToAll,
  userChannel,
} from "@/lib/events/publisher";

export async function acceptInviteAction(
  token: string,
): Promise<{ error: string | null }> {
  const session = await getServerSession();
  if (!session) {
    redirect(`/auth/login?returnTo=/invite/${encodeURIComponent(token)}`);
    return { error: "Authentication required." };
  }

  const limited = await rateLimit(sessionKey(session, "invites:accept"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  // Sent in no organization: whoever holds this link may belong to none yet,
  // and joining one is what it is for.
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/invites/accept`,
    {
      method: "POST",
      headers: { ...personHeaders(session), "content-type": "application/json" },
      body: JSON.stringify({ token }),
    },
  );
  if (!res) {
    return { error: "Organizations are unavailable right now — try again shortly." };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };

  const body: unknown = await res.json().catch(() => null);
  const data = body as {
    inviteId?: string;
    inviterEmail?: string;
    ownerOrganizationId?: string;
  } | null;

  await publishToAll(
    [
      session.email ? userChannel(session.email) : null,
      data?.inviterEmail ? userChannel(data.inviterEmail) : null,
      data?.ownerOrganizationId ? organizationChannel(data.ownerOrganizationId) : null,
    ],
    { type: "invite:resolved", data: { inviteId: data?.inviteId || "" } },
  );

  // The page goes on to `/console`, which asks `/me` for the cookie's
  // organization. A new invitee has no cookie, and `/me` would fall back to
  // their default, the organization their own sign-in provisioned; the one
  // they just joined is where they are going.
  if (data?.ownerOrganizationId) await setActiveOrganizationCookie(data.ownerOrganizationId);

  revalidatePath("/(app)/[organization]/projects", "page");
  return { error: null };
}
