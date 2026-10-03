"use server";

import { revalidatePath } from "next/cache";
import { redirect } from "next/navigation";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { organizationChannel, publishEvent } from "@/lib/events/publisher";
import { identityContext, organizationHeaders } from "@/lib/server/entities/identity-context";
import { activeOrganization, getServerContext } from "@/lib/server/entities/organization";
import { SWITCHED_ORGANIZATION, unplacedOrganization } from "@/lib/server/identity";
import { MAX_ORGANIZATION_NAME } from "@/lib/organization-name";
import { getServerSession } from "@/lib/server/session";

const Renamed = z.object({ slug: z.string() });

/// `movedTo` is the slug the organization goes by now, when the rename moved
/// it: the page follows it there, and so does everybody else with one of the
/// organization's pages open, who is told. Nothing is revalidated then — the
/// path this was posted from names no organization any more, and rendering it
/// again would answer "not found" before the page had moved.
export async function renameOrganizationAction(
  organizationId: string,
  name: string,
): Promise<{ error: string | null; movedTo?: string }> {
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
  if (!ctx) return { error: await unplacedOrganization() };
  if (ctx.organizationId !== organizationId) return { error: SWITCHED_ORGANIZATION };
  const gate = await getServerContext();
  const was = gate ? activeOrganization(gate)?.slug : undefined;

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

  const renamed = Renamed.safeParse(await res.json().catch(() => null));
  if (renamed.success && was !== undefined && renamed.data.slug !== was) {
    await publishEvent(organizationChannel(organizationId), {
      type: "slug:moved",
      data: { organizationId, from: was, to: renamed.data.slug },
    });
    return { error: null, movedTo: renamed.data.slug };
  }
  revalidatePath("/", "layout");
  return { error: null };
}
