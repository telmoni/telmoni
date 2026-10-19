"use server";

import { refresh } from "next/cache";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import {
  identityContext,
  organizationHeaders,
} from "@/lib/server/entities/identity-context";
import { SWITCHED_ORGANIZATION, unplacedOrganization } from "@/lib/server/identity";
import { NOT_DRAWN, NewestDrawn } from "@/lib/server/newest-drawn";
import { getServerSession } from "@/lib/server/session";

/// Mark the organization's own feed read up to `before`, the newest notice
/// the overview drew: the organization it rendered, which the action refuses
/// to mistake for another, and nothing that landed since.
export async function markOrganizationReadAction(
  organizationId: string,
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

  const ctx = await identityContext();
  if (!ctx) return { error: await unplacedOrganization() };
  if (ctx.organizationId !== organizationId) return { error: SWITCHED_ORGANIZATION };

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/notifications/read`, {
    method: "POST",
    headers: { ...organizationHeaders(ctx), "content-type": "application/json" },
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
