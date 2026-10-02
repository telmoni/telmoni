"use server";

import { revalidatePath } from "next/cache";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { organizationChannel, publishEvent } from "@/lib/events/publisher";
import {
  identityContext,
  projectHeaders,
} from "@/lib/server/entities/identity-context";
import { activeOrganization, getServerContext } from "@/lib/server/entities/organization";
import { fetchProject } from "@/lib/server/entities/projects";

import { getServerSession } from "@/lib/server/session";

interface ActionResult {
  error: string | null;
}

const Renamed = z.object({ slug: z.string() });

/// `movedTo` is the slug the project goes by now, when the rename moved it:
/// the page follows it there, and so does everybody else with one of the
/// project's pages open, who is told. Nothing is revalidated then — the path
/// this was posted from names no project any more, and rendering it again
/// would answer "not found" before the page had moved.
export async function updateProjectNameAction(
  projectId: string,
  name: string,
): Promise<ActionResult & { movedTo?: string }> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again." };
  const limited = await rateLimit(sessionKey(session, "project:rename"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const trimmed = name.trim();
  if (!trimmed) {
    return { error: "Project name cannot be empty." };
  }
  if (trimmed.length > 100) {
    return { error: "Project name must be 100 characters or fewer." };
  }

  const ctx = await identityContext();
  if (!ctx) return { error: "Unable to resolve session." };
  const was = (await fetchProject(projectId))?.slug;
  const gate = await getServerContext();
  const organization = gate ? activeOrganization(gate)?.slug : undefined;

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/projects/${projectId}`,
    {
      method: "PATCH",
      headers: {
        ...projectHeaders(ctx, projectId),
        "content-type": "application/json",
      },
      body: JSON.stringify({ name: trimmed }),
    },
  );

  if (!res || !res.ok) {
    const error = res
      ? (await extractProblem(res)).message
      : "Failed to update project name. Please try again.";
    return { error };
  }

  const renamed = Renamed.safeParse(await res.json().catch(() => null));
  if (renamed.success && was !== undefined && renamed.data.slug !== was) {
    if (organization !== undefined) {
      await publishEvent(organizationChannel(ctx.organizationId), {
        type: "slug:moved",
        data: {
          organizationId: ctx.organizationId,
          organization,
          projectId,
          from: was,
          to: renamed.data.slug,
        },
      });
    }
    return { error: null, movedTo: renamed.data.slug };
  }
  revalidatePath("/(app)/[organization]/[project]", "layout");
  return { error: null };
}

export async function deleteProjectAction(projectId: string): Promise<ActionResult> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again." };
  const limited = await rateLimit(sessionKey(session, "project:delete"), {
    limit: 3,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const ctx = await identityContext();
  if (!ctx) return { error: "Unable to resolve session." };

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/projects/${projectId}`,
    {
      method: "DELETE",
      headers: projectHeaders(ctx, projectId),
    },
  );

  if (!res || !res.ok) {
    const error = res
      ? (await extractProblem(res)).message
      : "Failed to delete the project. Please try again.";
    return { error };
  }

  // Every console page lists the organization's projects in its rail.
  revalidatePath("/", "layout");
  return { error: null };
}
