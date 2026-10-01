"use server";

import { redirect } from "next/navigation";
import { revalidatePath } from "next/cache";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { setActiveOrganizationCookie } from "@/lib/server/cookies";
import {
  identityContext,
  organizationHeaders,
} from "@/lib/server/entities/identity-context";
import { getServerContext } from "@/lib/server/entities/organization";
import type { Project } from "@/lib/server/entities/projects";
import { getServerSession } from "@/lib/server/session";
import { asRole } from "@/lib/types/enums";

const PROJECT_ID = /^project_[0-9A-Za-z]{16}$/;

export async function switchActiveOrganizationAction(
  targetOrganizationId: string,
  projectId?: string,
): Promise<void> {
  const session = await getServerSession();
  if (!session) redirect("/auth/login");

  const ctx = await getServerContext();
  if (!ctx) redirect("/console");

  // Every organization the person is in is an entry, their own included; a
  // cookie naming anything else would be ignored by auth anyway, and a switch
  // that silently does nothing looks like it worked.
  if (!ctx.organizations.some((o) => o.organizationId === targetOrganizationId)) {
    throw new Error("Unauthorized organization switch");
  }

  await setActiveOrganizationCookie(targetOrganizationId);

  // ⚠ **An ORGANIZATION row lands on the organization.** This was `/console`,
  // which resolves the caller's project listing and redirects to the FIRST project —
  // the right landing for a sign-in, and the wrong one here. Somebody who
  // picked an organization out of the switcher was dropped inside one of its
  // projects and had to reopen the switcher to reach the thing they had just
  // clicked. One rule for every row: an organization row goes to the
  // organization, a project row goes to the project.
  //
  // ⚠ **No trail here, unlike the rows for the ACTIVE organization.** Those
  // return you to the page you were last on in a resource, and that is a
  // per-tab client value (`use-console-trail.ts`) this server action cannot
  // see. Switching organizations is also the one navigation that changes what
  // you are allowed to read, so the overview is the honest landing.
  //
  // A malformed `projectId` lands here too. It is a caller-supplied value that
  // failed `PROJECT_ID`, so there is no project to honour — and the organization is
  // what the caller picked either way.
  redirect(projectId && PROJECT_ID.test(projectId) ? `/${projectId}` : "/organization");
}

const MAX_PROJECT_NAME = 100;

const CreatedProjectSchema = z.object({
  id: z.string(),
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
  if (!ctx) return { error: "Unable to resolve session." };

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

  // ⚠ **The console has to FOLLOW the project, or the caller lands on a 404.**
  // The project is created in `target`, but the active-organization cookie still
  // points at wherever they were standing, and `[projectId]/layout.tsx` resolves
  // the listing for THAT organization — so the project they just made is not in
  // it and `router.push(\`/${project.id}\`)` opens "Not found". The gate above is
  // what makes this safe to write; the read side re-checks membership anyway.
  if (target !== ctx.organizationId) {
    await setActiveOrganizationCookie(target);
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
      name: parsed.data.name,
      role: asRole(parsed.data.role),
    },
  };
}
