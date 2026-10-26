"use server";

import { refresh } from "next/cache";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { sessionHeaders } from "@/lib/server/entities/identity-context";
import { activeProjectForMutation } from "@/lib/server/identity";
import { getServerSession } from "@/lib/server/session";
import { Verdict } from "@/lib/types/enums";

const VoteSchema = z.strictObject({ incidentId: z.guid(), verdict: z.enum(Verdict) });

/// The person's word on one of the project's incidents, useful or noise,
/// given or changed. A Server Action is a POST, so the page a notice links
/// to casts nothing until the person picks a word on it.
export async function voteIncidentAction(
  projectId: string,
  incidentId: string,
  verdict: string,
): Promise<{ error: string | null }> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again." };
  // Posted, like the project: an incident id becomes part of the lane's path.
  const posted = VoteSchema.safeParse({ incidentId, verdict });
  if (!posted.success) return { error: "Couldn't tell which vote this is. Reload and try again." };

  const limited = await rateLimit(sessionKey(session, "incidents:vote"), {
    limit: 20,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const resolved = await activeProjectForMutation(projectId);
  if (resolved.error !== null) return { error: resolved.error };

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/telemetry/incidents/${posted.data.incidentId}/vote`,
    {
      method: "POST",
      headers: {
        ...sessionHeaders(session, resolved.organizationId, resolved.projectId),
        "content-type": "application/json",
      },
      body: JSON.stringify({ verdict: posted.data.verdict }),
    },
  );
  if (!res) return { error: "Alerts are unavailable right now — try again shortly." };
  if (!res.ok) return { error: (await extractProblem(res)).message };
  refresh();
  return { error: null };
}
