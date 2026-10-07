"use server";

import { revalidatePath } from "next/cache";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { publishEvent, userChannel } from "@/lib/events/publisher";
import { FlagOffDetail } from "@/lib/flags";
import { logger } from "@/lib/logger";
import { MAX_ORGANIZATION_NAME } from "@/lib/organization-name";
import {
  identityContext,
  organizationHeaders,
  personHeaders,
} from "@/lib/server/entities/identity-context";
import { getServerContext } from "@/lib/server/entities/organization";
import type { Project } from "@/lib/server/entities/projects";
import { FeatureOffProblemSchema } from "@/lib/server/flags";
import { unplacedOrganization } from "@/lib/server/identity";
import { getServerSession } from "@/lib/server/session";
import { SLUG_MAX_LENGTH, isOrganizationSlug, isSlug, organizationPath } from "@/lib/slug";
import { asRole } from "@/lib/types/enums";

const MAX_PROJECT_NAME = 100;

const CreatedProjectSchema = z.object({
  id: z.string(),
  slug: z.string(),
  name: z.string(),
  role: z.string(),
});

export interface CreateProjectResult {
  error: string | null;
  project?: Project;
}

export async function createProjectAction(
  name: string,
  organizationId?: string,
): Promise<CreateProjectResult> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again." };
  const limited = await rateLimit(sessionKey(session, "project:create"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const trimmed = name.trim();
  if (!trimmed) return { error: "Project name cannot be empty." };
  if (trimmed.length > MAX_PROJECT_NAME) {
    return { error: `Project name must be ${MAX_PROJECT_NAME} characters or fewer.` };
  }

  const ctx = await identityContext();
  if (!ctx) return { error: await unplacedOrganization() };

  // ⚠ **The organization is the caller's to CHOOSE and ours to CHECK.** The
  // project used to land wherever the active-organization cookie pointed, which
  // is wrong for somebody who administers more than one: the only place to
  // decide is the dialog, and the dialog can only offer a choice if this
  // accepts one.
  //
  // The console names `x-organization-id`, so naming one the caller may not
  // act in would be asserting a claim on their behalf. Auth checks
  // `can_create_projects` again on its own side — this half decides what the
  // console is willing to ASSERT, which is a different question from what auth
  // is willing to allow.
  let target = ctx.organizationId;
  if (organizationId && organizationId !== ctx.organizationId) {
    const gate = await getServerContext();
    const administers =
      gate?.organizations.some(
        (o) =>
          o.organizationId === organizationId && (o.role === "owner" || o.role === "admin"),
      ) ?? false;
    if (!administers) {
      return { error: "You cannot create a project in that organization." };
    }
    target = organizationId;
  }

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/projects`,
    {
      method: "POST",
      headers: {
        ...organizationHeaders({ ...ctx, organizationId: target }),
        "content-type": "application/json",
      },
      body: JSON.stringify({ name: trimmed }),
    },
  );

  if (!res || !res.ok) {
    const error = res
      ? (await extractProblem(res)).message
      : "Failed to create the project. Please try again.";
    return { error };
  }

  revalidatePath("/", "layout");

  const parsed = CreatedProjectSchema.safeParse(await res.json().catch(() => null));
  if (!parsed.success) {
    logger.warn(
      { fetcher: "createProjectAction", issues: parsed.error.issues },
      "entities: upstream shape mismatch",
    );
    return { error: null };
  }

  return {
    error: null,
    project: {
      id: parsed.data.id,
      slug: parsed.data.slug,
      name: parsed.data.name,
      role: asRole(parsed.data.role),
    },
  };
}

// The new organization's path is spelled with its slug, so it is held to a
// slug's shape before anything is redirected to it.
const CreatedOrganizationSchema = z.object({
  id: z.string(),
  slug: z.string().refine(isSlug),
  name: z.string(),
});

/// The dialog's field a refusal belongs under, as the organization's Settings
/// place them; none for one that is about neither.
export type CreateOrganizationField = "name" | "slug";

export interface CreateOrganizationResult {
  error: string | null;
  field?: CreateOrganizationField;
  /// The new organization's Overview, for the dialog to open. Not a
  /// `redirect()`: the switcher's dialog lives in a layout the move keeps, and
  /// would be left open over the page it moved to.
  href?: string;
}

/// Found another organization, owned by the caller. It names no organization
/// — the new one has no id until auth mints it — so it goes with the person's
/// own headers, as choosing a default organization does, and moves no default.
/// A blank URL is left to auth, which derives it from the name as it derives a
/// first organization's.
export async function createOrganizationAction(
  name: string,
  slug: string,
): Promise<CreateOrganizationResult> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again." };
  const limited = await rateLimit(sessionKey(session, "organization:create"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const trimmed = name.trim();
  if (!trimmed) return { error: "Give your organization a name.", field: "name" };
  if ([...trimmed].length > MAX_ORGANIZATION_NAME) {
    return { error: `Keep it to ${MAX_ORGANIZATION_NAME} characters or fewer.`, field: "name" };
  }
  const url = slug.trim();
  if (url && !isSlug(url)) {
    return {
      error: `A URL is lowercase letters and digits, in words joined by single hyphens, at most ${SLUG_MAX_LENGTH} characters.`,
      field: "slug",
    };
  }
  if (url && !isOrganizationSlug(url)) {
    return { error: `${url} is a word the console's own pages use — choose another.`, field: "slug" };
  }

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/organizations`, {
    method: "POST",
    headers: { ...personHeaders(session), "content-type": "application/json" },
    body: JSON.stringify(url ? { name: trimmed, slug: url } : { name: trimmed }),
  });
  if (!res) {
    return { error: "Organizations are unavailable right now — try again shortly." };
  }
  if (!res.ok) {
    if (res.status === 401) return { error: "Your session expired — sign in again." };
    const { message, problem } = await extractProblem(res);
    const off = FeatureOffProblemSchema.safeParse(problem);
    if (off.success) {
      const sentence: string | undefined = (FlagOffDetail as Record<string, string>)[off.data.flag];
      return { error: sentence ?? message };
    }
    // The one conflict a creation meets is the URL asked for; one with no URL
    // asked for belongs under neither field.
    if (res.status === 409) return url ? { error: message, field: "slug" } : { error: message };
    // Every URL auth refuses for its shape or its word is refused above by
    // the same rules (`lib/slug.ts`, whose words and length `contract.test.ts`
    // pins to auth's), so a 400 that gets this far is the name's: one auth
    // cleans to nothing.
    if (res.status === 400) return { error: message, field: "name" };
    return { error: message };
  }

  const created = CreatedOrganizationSchema.safeParse(await res.json().catch(() => null));
  if (!created.success) {
    logger.warn(
      { fetcher: "createOrganizationAction", issues: created.error.issues },
      "entities: upstream shape mismatch",
    );
    revalidatePath("/", "layout");
    return { error: null };
  }

  // The person's other tabs list it in their switcher once they read `/me`
  // again, which `ownership:changed` makes them do, as it does when an
  // organization changes hands. It goes on the person's own channel, as an
  // offer made to them does, since no tab subscribes to the new one's yet.
  if (session.email) {
    await publishEvent(userChannel(session.email), {
      type: "ownership:changed",
      data: { organizationId: created.data.id },
    });
  }
  revalidatePath("/", "layout");
  return { error: null, href: organizationPath(created.data.slug) };
}
