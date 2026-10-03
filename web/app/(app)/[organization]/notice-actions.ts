"use server";

import { redirect } from "next/navigation";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import {
  identityContext,
  organizationHeaders,
} from "@/lib/server/entities/identity-context";
import { SWITCHED_ORGANIZATION, unplacedOrganization } from "@/lib/server/identity";
import { getServerSession } from "@/lib/server/session";

/// Mark the organization's own feed read: the organization the overview
/// rendered, which the action refuses to mistake for another.
export async function markOrganizationReadAction(organizationId: string): Promise<{
  error: string | null;
}> {
  const session = await getServerSession();
  if (!session) redirect("/auth/login");

  const limited = await rateLimit(sessionKey(session, "notifications:read"), {
    limit: 20,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const ctx = await identityContext();
  if (!ctx) return { error: await unplacedOrganization() };
  if (ctx.organizationId !== organizationId) return { error: SWITCHED_ORGANIZATION };

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/notifications/read`,
    { method: "POST", headers: organizationHeaders(ctx) },
  );
  if (!res) {
    return {
      error: "Notifications are unavailable right now — try again shortly.",
    };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };
  return { error: null };
}
