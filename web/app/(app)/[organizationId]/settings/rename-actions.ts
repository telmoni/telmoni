"use server";

import { revalidatePath } from "next/cache";
import { redirect } from "next/navigation";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { identityContext, organizationHeaders } from "@/lib/server/entities/identity-context";
import { activeOrganization, getServerContext } from "@/lib/server/data";
import { organizationMatches } from "@/lib/slug";
import { SWITCHED_ORGANIZATION } from "@/lib/server/identity";
import { MAX_ORGANIZATION_NAME } from "@/lib/organization-name";
import { getServerSession } from "@/lib/server/session";

export async function renameOrganizationAction(
  organizationId: string,
  name: string,
): Promise<{ error: string | null }> {
  const session = await getServerSession();
  if (!session) redirect("/auth/login");

  const limited = await rateLimit(sessionKey(session, "organization:rename"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const trimmed = name.trim();
  if (!trimmed) return { error: "Give your organization a name." };
  if ([...trimmed].length > MAX_ORGANIZATION_NAME) {
    return {
      error: `Keep it to ${MAX_ORGANIZATION_NAME} characters or fewer.`,
    };
  }

  const ctx = await identityContext();
  if (!ctx) {
    return { error: "Couldn't resolve your organization right now. Try again in a moment." };
  }
  let matches = ctx.organizationId === organizationId;
  if (!matches) {
    try {
      const serverCtx = await getServerContext();
      const active = serverCtx ? activeOrganization(serverCtx) : null;
      if (active && organizationMatches(active, organizationId)) {
        matches = true;
      }
    } catch {
      // ignore
    }
  }
  if (!matches) return { error: SWITCHED_ORGANIZATION };

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/organization/name`,
    {
      method: "PUT",
      headers: { ...organizationHeaders(ctx), "content-type": "application/json" },
      body: JSON.stringify({ name: trimmed }),
    },
  );
  if (!res) {
    return { error: "Organizations are unavailable right now — try again shortly." };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };

  revalidatePath("/", "layout");
  return { error: null };
}
