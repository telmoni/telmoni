"use server";

import { revalidatePath } from "next/cache";
import { z } from "zod";

import { type ActionResult, type Refused, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { ProblemType } from "@/lib/api/problem";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { publishEvent, userChannel } from "@/lib/events/publisher";
import { createOrganizationElsewhere } from "@/lib/extension/create-organization";
import { createdOrganizationHref } from "@/lib/extension/created-organization";
import { FlagOffDetail } from "@/lib/flags";
import { logger } from "@/lib/logger";
import { checkOrganizationName, checkOrganizationUrl, checkProjectName } from "@/lib/names";
import { administers } from "@/lib/roles";
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
import { isSlug, organizationPath } from "@/lib/slug";
import { Role } from "@/lib/types/enums";

const CreatedProjectSchema = z.object({
  id: z.string(),
  slug: z.string(),
  name: z.string(),
  role: z.enum(Role),
});

/** Done with the project as listed, or with none where its answer could not
 *  be read: it was still made. */
export type CreateProjectResult = ActionResult<{ project?: Project }>;

export async function createProjectAction(
  name: string,
  organizationId?: string,
): Promise<CreateProjectResult> {
  const session = await getServerSession();
  if (!session) return refused("Your session expired — sign in again.");
  const limited = await rateLimit(sessionKey(session, "project:create"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return refused("Too many requests — slow down a moment.");

  const checked = checkProjectName(name);
  if (!checked.ok) return refused(checked.error);
  const trimmed = checked.value;

  const ctx = await identityContext();
  if (!ctx) return refused(await unplacedOrganization());

  // ⚠ **The organization is the caller's to NAME and ours to CHECK**, never
  // the active-organization cookie's, which follows the last tab that moved:
  // the panel names the organization its own page stands in.
  //
  // The console names `x-organization-id`, so naming one the caller may not
  // act in would be asserting a claim on their behalf. Auth checks
  // `can_create_projects` again on its own side — this half decides what the
  // console is willing to ASSERT, which is a different question from what auth
  // is willing to allow.
  let target = ctx.organizationId;
  if (organizationId && organizationId !== ctx.organizationId) {
    const gate = await getServerContext();
    const mayCreate =
      gate?.organizations.some((o) => o.organizationId === organizationId && administers(o.role)) ??
      false;
    if (!mayCreate) {
      return refused("You cannot create a project in that organization.");
    }
    target = organizationId;
  }

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/projects`,
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
    return refused(error);
  }

  revalidatePath("/", "layout");

  const parsed = CreatedProjectSchema.safeParse(await res.json().catch(() => null));
  if (!parsed.success) {
    logger.warn(
      { fetcher: "createProjectAction", issues: parsed.error.issues },
      "entities: upstream shape mismatch",
    );
    return { ok: true };
  }

  return { ok: true, project: parsed.data };
}

// The new organization's path is spelled with its slug, so it is held to a
// slug's shape before anything is redirected to it.
const CreatedOrganizationSchema = z.object({
  id: z.string(),
  slug: z.string().refine(isSlug),
  name: z.string(),
});

/// The panel's field a refusal belongs under; none for one about neither.
export type CreateOrganizationField = "name" | "slug";

/// Done with the new organization's Overview, or wherever a console built on
/// this one sends it (`lib/extension/created-organization.ts`), for the panel
/// to open: a `redirect()` from the layout's panel remounts the console shell.
/// A refusal may name the field it is about.
export type CreateOrganizationResult =
  | ActionResult<{ href?: string }>
  | (Refused & { field?: CreateOrganizationField });

/// Found another organization, owned by the caller. It names no organization
/// — the new one has no id until auth mints it — so it goes with the person's
/// own headers, as choosing a default organization does, and moves no default.
/// A blank URL is left to auth, which derives it from the name as it derives a
/// first organization's.
export async function createOrganizationAction(
  name: string,
  slug: string,
  extras: Record<string, string> = {},
): Promise<CreateOrganizationResult> {
  const session = await getServerSession();
  if (!session) return refused("Your session expired — sign in again.");
  const limited = await rateLimit(sessionKey(session, "organization:create"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return refused("Too many requests — slow down a moment.");

  const checkedName = checkOrganizationName(name);
  if (!checkedName.ok) return { ...refused(checkedName.error), field: "name" };
  const trimmed = checkedName.value;
  // No URL asked for is auth's to make from the name.
  const url = slug.trim();
  if (url) {
    const checkedUrl = checkOrganizationUrl(url);
    if (!checkedUrl.ok) return { ...refused(checkedUrl.error), field: "slug" };
  }

  // A console built on this one may take the founding elsewhere first — one
  // that sells plans has a second organization paid for before it exists —
  // and answers where to send the person, or what refused it; `null` founds
  // it here.
  const elsewhere = await createOrganizationElsewhere({ name: trimmed, slug: url, extras });
  if (elsewhere) {
    return "href" in elsewhere
      ? { ok: true, href: elsewhere.href }
      : { ...refused(elsewhere.error), field: elsewhere.field };
  }

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/organizations`, {
    method: "POST",
    headers: { ...personHeaders(session), "content-type": "application/json" },
    body: JSON.stringify(url ? { name: trimmed, slug: url } : { name: trimmed }),
  });
  if (!res) {
    return refused("Organizations are unavailable right now — try again shortly.");
  }
  if (!res.ok) {
    if (res.status === 401) return refused("Your session expired — sign in again.");
    const { message, problem } = await extractProblem(res);
    const off = FeatureOffProblemSchema.safeParse(problem);
    if (off.success) return refused(FlagOffDetail[off.data.flag]);
    // The one conflict a creation meets is the URL asked for; one with no URL
    // asked for belongs under neither field.
    if (res.status === 409) return url ? { ...refused(message), field: "slug" } : refused(message);
    // Auth types a URL's refusal as the URL's: one for a word no URL may
    // hold, which this console keeps no list of — its shape and the console's
    // own words are refused above, by rules `contract.test.ts` pins to
    // auth's. Any other 400 is the name's: one auth cleans to nothing.
    if (res.status === 400) {
      return {
        ...refused(message),
        field: problem?.type === ProblemType.BadUrl ? "slug" : "name",
      };
    }
    return refused(message);
  }

  const created = CreatedOrganizationSchema.safeParse(await res.json().catch(() => null));
  if (!created.success) {
    logger.warn(
      { fetcher: "createOrganizationAction", issues: created.error.issues },
      "entities: upstream shape mismatch",
    );
    revalidatePath("/", "layout");
    return { ok: true };
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
  // Its Overview, unless a console built on this one sends it elsewhere.
  const href = (await createdOrganizationHref(created.data)) ?? organizationPath(created.data.slug);
  return { ok: true, href };
}
