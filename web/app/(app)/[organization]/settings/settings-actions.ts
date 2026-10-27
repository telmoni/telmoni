"use server";

import { revalidatePath } from "next/cache";
import { redirect, RedirectType } from "next/navigation";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { organizationChannel, publishEvent } from "@/lib/events/publisher";
import { checkOrganizationName, checkOrganizationUrl } from "@/lib/names";
import { identityContext, organizationHeaders } from "@/lib/server/entities/identity-context";
import { activeOrganization, getServerContext } from "@/lib/server/entities/organization";
import { SWITCHED_ORGANIZATION, unplacedOrganization } from "@/lib/server/identity";
import { canonicalPath } from "@/lib/server/path";
import { getServerSession } from "@/lib/server/session";

const Updated = z.object({ slug: z.string() });

/// What the organization's two settings answer. A write that moved the
/// organization's slug — a URL changed on purpose — answers with the same page
/// at the new path instead, drawn there rather than here, where the old path
/// names no organization any more; everybody else with one of the
/// organization's pages open is told, and follows it too.
interface Answer {
  error: string | null;
}

/// `PATCH /internal/v1/organization` for the organization the page rendered: the
/// name, the slug, or both. The name and the URL are two settings: a name
/// moves no slug.
async function patchOrganization(
  organizationId: string,
  bucket: string,
  body: { name?: string; slug?: string },
): Promise<Answer> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again." };

  const limited = await rateLimit(sessionKey(session, bucket), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const ctx = await identityContext();
  if (!ctx) return { error: await unplacedOrganization() };
  if (ctx.organizationId !== organizationId) return { error: SWITCHED_ORGANIZATION };
  const gate = await getServerContext();
  const was = gate ? activeOrganization(gate)?.slug : undefined;

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/organization`, {
    method: "PATCH",
    headers: { ...organizationHeaders(ctx), "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res) {
    return { error: "Organizations are unavailable right now — try again shortly." };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };

  const updated = Updated.safeParse(await res.json().catch(() => null));
  if (updated.success && was !== undefined && updated.data.slug !== was) {
    await publishEvent(organizationChannel(organizationId), {
      type: "slug:moved",
      data: { organizationId, from: was, to: updated.data.slug },
    });
    // `replace`, so Back does not return to a path that names nothing.
    // Nothing is revalidated first: the new path is drawn whole with this
    // answer, and a revalidation would have the router read it all again as
    // soon as it lands.
    redirect(await canonicalPath([updated.data.slug]), RedirectType.replace);
  }
  revalidatePath("/", "layout");
  return { error: null };
}

/// Rename the organization from its Settings page. A name moves nothing: the
/// URL is a setting of its own.
export async function renameOrganizationAction(
  organizationId: string,
  name: string,
): Promise<Answer> {
  const checked = checkOrganizationName(name);
  if (!checked.ok) return { error: checked.error };
  return patchOrganization(organizationId, "organization:rename", { name: checked.value });
}

/// Change the slug the organization's paths begin with. Checked for shape
/// here so the form can say what a URL is; whether it is free, and the words
/// the console keeps for itself, are auth's to say.
export async function changeOrganizationUrlAction(
  organizationId: string,
  slug: string,
): Promise<Answer> {
  const checked = checkOrganizationUrl(slug);
  if (!checked.ok) return { error: checked.error };
  return patchOrganization(organizationId, "organization:url", { slug: checked.value });
}
