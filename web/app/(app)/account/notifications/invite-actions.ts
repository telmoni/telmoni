"use server";

import { refresh, revalidatePath } from "next/cache";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, readBody, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import {
  organizationChannel,
  publishToAll,
  userChannel,
} from "@/lib/events/publisher";
import { personHeaders } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";
import { type SettledInvite, SettledInviteSchema } from "@/lib/types/settled-invite";

function settledChannels(
  sessionEmail: string | undefined,
  data: SettledInvite | null,
): (string | null)[] {
  return [
    sessionEmail ? userChannel(sessionEmail) : null,
    data?.inviterEmail ? userChannel(data.inviterEmail) : null,
    data?.ownerOrganizationId ? organizationChannel(data.ownerOrganizationId) : null,
  ];
}

async function answer(res: Response | null): Promise<ActionResult> {
  if (!res) {
    return refused("Organizations are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);
  return { ok: true };
}

export async function acceptIncomingInviteAction(
  inviteId: string,
): Promise<ActionResult> {
  const session = await getServerSession();
  if (!session) return refused("Your session expired — sign in again.");
  const limited = await rateLimit(sessionKey(session, "members:accept-invite"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return refused("Too many requests — slow down a moment.");

  // The person's own lane, sent in no organization: an invitation is how
  // somebody in none gets into one.
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/me/invites/${encodeURIComponent(inviteId)}/accept`,
    {
      method: "POST",
      headers: personHeaders(session),
    },
  );
  if (!res) {
    return refused("Organizations are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);

  const data = await readBody(res, SettledInviteSchema);
  await publishToAll(settledChannels(session.email, data), {
    type: "invite:resolved",
    data: { inviteId: data?.inviteId || inviteId },
  });

  // Wherever it was answered — this page or the header's bell — the page is
  // read again for the projects the accept just granted: a revalidation in
  // an action draws the current route again, whichever path it names.
  revalidatePath("/(app)/[organization]/projects", "page");
  return { ok: true };
}

export async function declineIncomingInviteAction(
  inviteId: string,
): Promise<ActionResult> {
  const session = await getServerSession();
  if (!session) return refused("Your session expired — sign in again.");
  const limited = await rateLimit(sessionKey(session, "members:decline-invite"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return refused("Too many requests — slow down a moment.");

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/me/invites/${encodeURIComponent(inviteId)}/decline`,
    {
      method: "POST",
      headers: personHeaders(session),
    },
  );
  const ans = await answer(res);
  if (!ans.ok || !res) return ans;

  const data = await readBody(res, SettledInviteSchema);
  await publishToAll(settledChannels(session.email, data), {
    type: "invite:resolved",
    data: { inviteId: data?.inviteId || inviteId },
  });

  refresh();
  return ans;
}
