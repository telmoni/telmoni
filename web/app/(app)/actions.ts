"use server";

import { revalidatePath } from "next/cache";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import {
  identityContext,
  organizationHeaders,
} from "@/lib/server/entities/identity-context";
import { getServerContext } from "@/lib/server/entities/organization";
import type { Project } from "@/lib/server/entities/projects";
import { getServerSession } from "@/lib/server/session";
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
