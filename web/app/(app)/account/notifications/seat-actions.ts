"use server";

import { refresh } from "next/cache";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { isProjectId } from "@/lib/connect";
import { env } from "@/lib/env";
import { publishEvent, userChannel } from "@/lib/events/publisher";
import { personHeaders } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";
import { type ActionResult, refused } from "@/lib/action-result";

// The person has seen the seat given to them — opened the project from the
// bell, or dismissed the row — so the bell stops showing it, in their other
// consoles too, told on their own channel. The seat stays.
export async function markSeatSeenAction(projectId: string): Promise<ActionResult> {
  const session = await getServerSession();
  if (!session) return refused("Your session expired — sign in again.");
  const limited = await rateLimit(sessionKey(session, "seats:seen"), {
    limit: 30,
    windowMs: 60_000,
  });
  if (limited) return refused("Too many requests — slow down a moment.");
  // The id becomes a path segment of the upstream URL: anything but a project
  // id is refused before it can aim the request elsewhere.
  if (!isProjectId(projectId)) {
    return refused("Couldn't resolve that project. Reload and try again.");
  }
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/me/seats/${encodeURIComponent(projectId)}/seen`,
    { method: "POST", headers: personHeaders(session) },
  );
  if (!res) {
    return refused("Organizations are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);
  const body: unknown = await res.json().catch(() => null);
  const organizationId = (body as { organizationId?: unknown } | null)?.organizationId;
  if (typeof organizationId === "string") {
    await publishEvent(userChannel(session.email), {
      type: "membership:changed",
      data: { organizationId, projectId },
    });
  }
  refresh();
  return { ok: true };
}
