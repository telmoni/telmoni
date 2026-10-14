"use server";

import { refresh } from "next/cache";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { sessionHeaders } from "@/lib/server/entities/identity-context";
import { activeProjectForMutation } from "@/lib/server/identity";
import { getServerSession } from "@/lib/server/session";

/// Mark the project's feed read: every unread notice on it, for everybody on
/// the project, as the service marks a feed — those that arrived after the
/// page was drawn among them. The project is the one the overview rendered,
/// by id, and the service answers whether the person is on it, so a stale
/// page marks no project it is not showing.
export async function markProjectReadAction(projectId: string): Promise<{
  error: string | null;
}> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again." };

  const limited = await rateLimit(sessionKey(session, "notifications:read"), {
    limit: 20,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const resolved = await activeProjectForMutation(projectId);
  if (resolved.error !== null) return { error: resolved.error };

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/notifications/read`, {
    method: "POST",
    headers: sessionHeaders(session, resolved.organizationId, resolved.projectId),
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
