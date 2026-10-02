"use server";

import { revalidatePath } from "next/cache";
import { redirect } from "next/navigation";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import {
  organizationChannel,
  publishToAll,
  userChannel,
} from "@/lib/events/publisher";
import { personHeaders } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

export interface ActionResult {
  error: string | null;
}

const SettledInvite = z
  .object({
    inviteId: z.string().optional(),
    inviterEmail: z.email().nullish(),
    ownerOrganizationId: z.string().optional(),
  })
  .partial()
  .passthrough();

type SettledInvite = z.infer<typeof SettledInvite>;

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
    return { error: "Organizations are unavailable right now — try again shortly." };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };
  return { error: null };
}

export async function acceptIncomingInviteAction(
  inviteId: string,
): Promise<ActionResult> {
  const session = await getServerSession();
  if (!session) {
    redirect("/auth/login");
    return { error: "Authentication required." };
  }
  const limited = await rateLimit(sessionKey(session, "members:accept-invite"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  // The person's own lane, sent in no organization: an invitation is how
  // somebody in none gets into one.
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/me/invites/${encodeURIComponent(inviteId)}/accept`,
    {
      method: "POST",
      headers: personHeaders(session),
    },
  );
  if (!res) {
    return { error: "Organizations are unavailable right now — try again shortly." };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };

  const body: unknown = await res.json().catch(() => null);
  const data = SettledInvite.safeParse(body).data ?? null;
  await publishToAll(settledChannels(session.email, data), {
    type: "invite:resolved",
    data: { inviteId: data?.inviteId || inviteId },
  });

  revalidatePath("/(app)/[organization]/~/projects", "page");
  return { error: null };
}

export async function declineIncomingInviteAction(
  inviteId: string,
): Promise<ActionResult> {
  const session = await getServerSession();
  if (!session) {
    redirect("/auth/login");
    return { error: "Authentication required." };
  }
  const limited = await rateLimit(sessionKey(session, "members:decline-invite"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/me/invites/${encodeURIComponent(inviteId)}/decline`,
    {
      method: "POST",
      headers: personHeaders(session),
    },
  );
  const ans = await answer(res);
  if (ans.error || !res) return ans;

  const body: unknown = await res.json().catch(() => null);
  const data = SettledInvite.safeParse(body).data ?? null;
  await publishToAll(settledChannels(session.email, data), {
    type: "invite:resolved",
    data: { inviteId: data?.inviteId || inviteId },
  });

  return ans;
}
