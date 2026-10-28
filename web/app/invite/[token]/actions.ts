"use server";

import { revalidatePath } from "next/cache";
import { redirect } from "next/navigation";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, readBody, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { admit } from "@/lib/server/admit";
import { setActiveOrganizationCookie } from "@/lib/server/cookies";
import { personHeaders } from "@/lib/server/entities/identity-context";
import { SettledInviteSchema } from "@/lib/types/settled-invite";
import {
  organizationChannel,
  publishToAll,
  userChannel,
} from "@/lib/events/publisher";

export async function acceptInviteAction(
  token: string,
): Promise<ActionResult> {
  // The page offers the signed-out its own links to sign in, back to it: a
  // session that ran out since it was drawn is told so.
  const gate = await admit({ call: "invites:accept", limit: 10 });
  if (!gate.ok) return refused(gate.error);
  const { session } = gate;

  // Sent in no organization: whoever holds this link may belong to none yet,
  // and joining one is what it is for.
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/invites/accept`,
    {
      method: "POST",
      headers: { ...personHeaders(session), "content-type": "application/json" },
      body: JSON.stringify({ token }),
    },
  );
  if (!res) {
    return refused("Organizations are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);

  const data = await readBody(res, SettledInviteSchema);

  await publishToAll(
    [
      session.email ? userChannel(session.email) : null,
      data?.inviterEmail ? userChannel(data.inviterEmail) : null,
      data?.ownerOrganizationId ? organizationChannel(data.ownerOrganizationId) : null,
    ],
    { type: "invite:resolved", data: { inviteId: data?.inviteId || "" } },
  );

  // On to `/console`, which asks `/me` for the cookie's organization. A new
  // invitee has no cookie, and `/me` would fall back to their default, the
  // organization their own sign-in provisioned; the one they just joined is
  // where they are going.
  if (data?.ownerOrganizationId) await setActiveOrganizationCookie(data.ownerOrganizationId);

  revalidatePath("/(app)/[organization]/projects", "page");
  redirect("/console");
}
