"use server";

import { revalidatePath } from "next/cache";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { isProjectId } from "@/lib/connect";
import { env } from "@/lib/env";
import { organizationChannel, publishEvent } from "@/lib/events/publisher";
import {
  identityContext,
  projectHeaders,
} from "@/lib/server/entities/identity-context";
import { activeOrganization, getServerContext } from "@/lib/server/entities/organization";
import { fetchProject } from "@/lib/server/entities/projects";
import { unplacedOrganization } from "@/lib/server/identity";
import { getServerSession } from "@/lib/server/session";

interface ActionResult {
  error: string | null;
}

const Renamed = z.object({ slug: z.string() });

const UNRESOLVED_PROJECT = "Couldn't resolve that project. Reload and try again.";

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

  // A Server Action is a public endpoint, and the id becomes a path segment
  // of the upstream URL: anything but a project id is refused before it can
  // aim the request at another lane.
  if (!isProjectId(projectId)) return { error: UNRESOLVED_PROJECT };

  const ctx = await identityContext();
  if (!ctx) return { error: await unplacedOrganization() };
  const was = (await fetchProject(projectId))?.slug;
  const gate = await getServerContext();
  const organization = gate ? activeOrganization(gate)?.slug : undefined;

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/projects/${encodeURIComponent(projectId)}`,
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
  if (!isProjectId(projectId)) return { error: UNRESOLVED_PROJECT };

  const ctx = await identityContext();
  if (!ctx) return { error: await unplacedOrganization() };

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/projects/${encodeURIComponent(projectId)}`,
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

  // Everybody in the organization is told, the owner and admins who hold no
  // seat included: a tab on the project leaves it, and every other draws its
  // rail again. The deleting tab leaves for the console's entry page itself.
  await publishEvent(organizationChannel(ctx.organizationId), {
    type: "membership:removed",
    data: { organizationId: ctx.organizationId, projectId },
  });

  // Every console page lists the organization's projects in its rail.
  revalidatePath("/", "layout");
  return { error: null };
}
