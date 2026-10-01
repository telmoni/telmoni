"use server";

import { redirect } from "next/navigation";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import {
  identityContext,
  organizationHeaders,
} from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

export async function markOrganizationReadAction(): Promise<{
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
  if (!ctx) {
    return {
      error: "Couldn't resolve your organization right now. Try again in a moment.",
    };
  }

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
