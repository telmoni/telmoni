"use server";

import { refresh, revalidatePath } from "next/cache";
import { redirect, RedirectType } from "next/navigation";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { isProjectId } from "@/lib/connect";
import { env } from "@/lib/env";
import { organizationChannel, publishEvent } from "@/lib/events/publisher";
import { checkProjectName } from "@/lib/names";
import {
  identityContext,
  projectHeaders,
} from "@/lib/server/entities/identity-context";
import { activeOrganization, getServerContext } from "@/lib/server/entities/organization";
import { fetchProject } from "@/lib/server/entities/projects";
import { unplacedOrganization } from "@/lib/server/identity";
import { canonicalPath } from "@/lib/server/path";
import { getServerSession } from "@/lib/server/session";
import { ContentMode } from "@/lib/types/enums";

const Renamed = z.object({ slug: z.string() });
const Mode = z.enum(ContentMode);

const UNRESOLVED_PROJECT = "Couldn't resolve that project. Reload and try again.";

/// A rename moves the project's slug with its name, and the page follows it:
/// the answer is the same page at the project's new path, drawn there rather
/// than here, where the old path names no project any more. Everybody else
/// with one of the project's pages open is told, and follows it too.
export async function updateProjectNameAction(
  projectId: string,
  name: string,
): Promise<ActionResult> {
  const session = await getServerSession();
  if (!session) return refused("Your session expired — sign in again.");
  const limited = await rateLimit(sessionKey(session, "project:rename"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return refused("Too many requests — slow down a moment.");

  const checked = checkProjectName(name);
  if (!checked.ok) return refused(checked.error);
  const trimmed = checked.value;

  // A Server Action is a public endpoint, and the id becomes a path segment
  // of the upstream URL: anything but a project id is refused before it can
  // aim the request at another lane.
  if (!isProjectId(projectId)) return refused(UNRESOLVED_PROJECT);

  const ctx = await identityContext();
  if (!ctx) return refused(await unplacedOrganization());
  const was = (await fetchProject(projectId))?.slug;
  const gate = await getServerContext();
  const organization = gate ? activeOrganization(gate)?.slug : undefined;

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/projects/${encodeURIComponent(projectId)}`,
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
    return refused(error);
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
    // `replace`, so Back does not return to a path that names nothing. With
    // no organization to spell the path with, the console's own way in finds
    // the project's. Nothing is revalidated first: the new path is drawn
    // whole with this answer, and a revalidation would have the router read
    // it all again as soon as it lands.
    redirect(
      organization !== undefined
        ? await canonicalPath([organization, renamed.data.slug])
        : "/console",
      RedirectType.replace,
    );
  }
  revalidatePath("/(app)/[organization]/[project]", "layout");
  return { ok: true };
}

/// The server decides who may (the organization's owner alone) and which modes
/// are open; this only refuses what could never reach it.
export async function updateContentModeAction(
  projectId: string,
  mode: ContentMode,
): Promise<ActionResult> {
  const session = await getServerSession();
  if (!session) return refused("Your session expired — sign in again.");
  const limited = await rateLimit(sessionKey(session, "project:content-mode"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return refused("Too many requests — slow down a moment.");
  if (!isProjectId(projectId)) return refused(UNRESOLVED_PROJECT);
  // A Server Action takes whatever a caller posts, whatever its type says.
  const chosen = Mode.safeParse(mode);
  if (!chosen.success) return refused("That is not a content mode.");

  const ctx = await identityContext();
  if (!ctx) return refused(await unplacedOrganization());

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/content-mode`, {
    method: "PUT",
    headers: {
      ...projectHeaders(ctx, projectId),
      "content-type": "application/json",
    },
    body: JSON.stringify({ content_mode: chosen.data }),
  });

  if (!res) return refused("Failed to change the content mode. Please try again.");
  if (!res.ok) {
    // A refusal can mean the page is out of date — the owner changed, or a
    // mode closed — so it is drawn again with the answer.
    refresh();
    return refused((await extractProblem(res)).message);
  }

  revalidatePath("/(app)/[organization]/[project]/settings", "page");
  return { ok: true };
}

export async function deleteProjectAction(projectId: string): Promise<ActionResult> {
  const session = await getServerSession();
  if (!session) return refused("Your session expired — sign in again.");
  const limited = await rateLimit(sessionKey(session, "project:delete"), {
    limit: 3,
    windowMs: 60_000,
  });
  if (limited) return refused("Too many requests — slow down a moment.");
  if (!isProjectId(projectId)) return refused(UNRESOLVED_PROJECT);

  const ctx = await identityContext();
  if (!ctx) return refused(await unplacedOrganization());

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/projects/${encodeURIComponent(projectId)}`,
    {
      method: "DELETE",
      headers: projectHeaders(ctx, projectId),
    },
  );

  if (!res || !res.ok) {
    const error = res
      ? (await extractProblem(res)).message
      : "Failed to delete the project. Please try again.";
    return refused(error);
  }

  // Everybody in the organization is told, the owner and admins who hold no
  // seat included: a tab on the project leaves it, and every other draws its
  // rail again. The deleting tab leaves for the console's entry page itself.
  await publishEvent(organizationChannel(ctx.organizationId), {
    type: "membership:removed",
    data: { organizationId: ctx.organizationId, projectId },
  });

  // Every console page lists the organization's projects in its header's
  // switcher, so each one visited is read again; this one, the deleted
  // project's own, is left at once for the console's entry page, whose page
  // comes with this answer — `replace`, so Back does not return to it.
  revalidatePath("/", "layout");
  redirect("/console", RedirectType.replace);
}
