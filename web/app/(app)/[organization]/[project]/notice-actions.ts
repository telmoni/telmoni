"use server";

import { refresh } from "next/cache";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { sessionHeaders } from "@/lib/server/entities/identity-context";
import { activeProjectForMutation } from "@/lib/server/identity";
import { NOT_DRAWN, NewestDrawn } from "@/lib/server/newest-drawn";
import { getServerSession } from "@/lib/server/session";

/// Mark the project's feed read up to `before`, the newest notice the page
/// drew: every unread notice to there, for everybody on the project, as the
/// service marks a feed, and none that arrived since. The project is the one
/// the overview rendered, by id, and the service answers whether the person
/// is on it, so a stale page marks no project it is not showing.
export async function markProjectReadAction(
  projectId: string,
  before: string,
): Promise<{ error: string | null }> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again." };
  // Posted, like the id: a time the page could not have drawn asks nothing.
  if (!NewestDrawn.safeParse(before).success) return { error: NOT_DRAWN };

  const limited = await rateLimit(sessionKey(session, "notifications:read"), {
    limit: 20,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const resolved = await activeProjectForMutation(projectId);
  if (resolved.error !== null) return { error: resolved.error };

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/notifications/read`, {
    method: "POST",
    headers: {
      ...sessionHeaders(session, resolved.organizationId, resolved.projectId),
      "content-type": "application/json",
    },
    body: JSON.stringify({ before }),
  });
  if (!res) {
    return {
      error: "Notifications are unavailable right now — try again shortly.",
    };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };
  refresh();
  return { error: null };
}
