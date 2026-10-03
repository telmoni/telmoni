"use server";

import { revalidatePath } from "next/cache";
import { redirect } from "next/navigation";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { organizationChannel, publishEvent } from "@/lib/events/publisher";
import { MAX_ORGANIZATION_NAME } from "@/lib/organization-name";
import {
  identityContext,
  organizationHeaders,
  sessionHeaders,
} from "@/lib/server/entities/identity-context";
import { activeOrganization, getServerContext } from "@/lib/server/entities/organization";
import { SWITCHED_ORGANIZATION, unplacedOrganization } from "@/lib/server/identity";
import { getServerSession } from "@/lib/server/session";
import { SLUG_MAX_LENGTH, isOrganizationSlug, isSlug } from "@/lib/slug";

const Updated = z.object({ slug: z.string() });

/// What the organization's two settings answer. `movedTo` is the slug the
/// organization goes by now, when the write moved it: a URL changed on
/// purpose, or the first name taking the organization off its placeholder.
/// The page follows it there, and so does everybody else with one of the
/// organization's pages open, who is told. Nothing is revalidated then — the
/// path this was posted from names no organization any more, and rendering
/// it again would answer "not found" before the page had moved.
interface Answer {
  error: string | null;
  movedTo?: string;
  /// The slug the organization goes by after the write, moved or not: the
  /// first-name form navigates to it, since the path it was posted from names
  /// no organization.
  slug?: string;
}

/// `PATCH /internal/organization` for the organization the page rendered: the
/// name, the slug, or both. The name and the URL are two settings, as on
/// Vercel's team page: a name moves no slug once the organization has a name.
async function patchOrganization(
  organizationId: string,
  bucket: string,
  body: { name?: string; slug?: string },
): Promise<Answer> {
  const session = await getServerSession();
  if (!session) redirect("/auth/login");

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

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/organization`, {
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
    return { error: null, movedTo: updated.data.slug };
  }
  revalidatePath("/", "layout");
  return { error: null };
}

/// Rename the organization from its Settings page. A name moves nothing once
/// the organization has one; the first name, which does, is
/// `nameOrganizationAction`'s.
export async function renameOrganizationAction(
  organizationId: string,
  name: string,
): Promise<Answer> {
  const trimmed = name.trim();
  if (!trimmed) return { error: "Give your organization a name." };
  if ([...trimmed].length > MAX_ORGANIZATION_NAME) {
    return {
      error: `Keep it to ${MAX_ORGANIZATION_NAME} characters or fewer.`,
    };
  }
  return patchOrganization(organizationId, "organization:rename", { name: trimmed });
}

/// The first name, posted from `/console`, which stands on no organization's
/// path: the request resolves the cookie's organization there, and that is
/// another one when the owner opened their unnamed organization from inside
/// it. So this acts on the organization the form was handed, under
/// `sessionHeaders`, and only when `/me` lists the person as its owner; auth
/// derives the role again from its own tables either way. The first name
/// takes the organization off its placeholder slug, so the answer carries
/// where its pages are now.
export async function nameOrganizationAction(
  organizationId: string,
  name: string,
): Promise<Answer> {
  const trimmed = name.trim();
  if (!trimmed) return { error: "Give your organization a name." };
  if ([...trimmed].length > MAX_ORGANIZATION_NAME) {
    return {
      error: `Keep it to ${MAX_ORGANIZATION_NAME} characters or fewer.`,
    };
  }

  const session = await getServerSession();
  if (!session) redirect("/auth/login");

  const limited = await rateLimit(sessionKey(session, "organization:name"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const ctx = await getServerContext();
  if (!ctx) return { error: await unplacedOrganization() };
  const owned = ctx.organizations.find(
    (o) => o.organizationId === organizationId && o.role === "owner",
  );
  if (!owned) return { error: "Only the organization's owner names it." };
  // A stale `/console` tab, posting after the name was given: a rename is
  // Settings', where it is seen as one.
  if (owned.name?.trim()) {
    return { error: "This organization already has a name; rename it in Settings." };
  }

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/organization`, {
    method: "PATCH",
    headers: { ...sessionHeaders(session, organizationId), "content-type": "application/json" },
    body: JSON.stringify({ name: trimmed }),
  });
  if (!res) {
    return { error: "Organizations are unavailable right now — try again shortly." };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };

  const updated = Updated.safeParse(await res.json().catch(() => null));
  if (!updated.success) return { error: null };
  if (updated.data.slug !== owned.slug) {
    await publishEvent(organizationChannel(organizationId), {
      type: "slug:moved",
      data: { organizationId, from: owned.slug, to: updated.data.slug },
    });
    return { error: null, movedTo: updated.data.slug, slug: updated.data.slug };
  }
  return { error: null, slug: updated.data.slug };
}

/// Change the slug the organization's paths begin with. Checked for shape
/// here so the form can say what a URL is; whether it is free, and the words
/// the console keeps for itself, are auth's to say.
export async function changeOrganizationUrlAction(
  organizationId: string,
  slug: string,
): Promise<Answer> {
  const trimmed = slug.trim();
  if (!isSlug(trimmed)) {
    return {
      error: `A URL is lowercase letters and digits, in words joined by single hyphens, at most ${SLUG_MAX_LENGTH} characters.`,
    };
  }
  if (!isOrganizationSlug(trimmed)) {
    return { error: `${trimmed} is a word the console's own pages use — choose another.` };
  }
  return patchOrganization(organizationId, "organization:url", { slug: trimmed });
}
