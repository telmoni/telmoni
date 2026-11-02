"use server";

import { refresh, revalidatePath } from "next/cache";
import { redirect, RedirectType } from "next/navigation";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { NOT_A_CODE, isConfirmationCode } from "@/lib/confirmation-code";
import { env } from "@/lib/env";
import { organizationChannel, publishEvent } from "@/lib/events/publisher";
import { checkProjectName } from "@/lib/names";
import { admitProject } from "@/lib/server/admit";
import { activeOrganization, getServerContext } from "@/lib/server/entities/organization";
import { ProjectRenamedSchema, fetchProject } from "@/lib/server/entities/projects";
import { ModelPricesSchema } from "@/lib/server/entities/model-prices";
import { canonicalPath } from "@/lib/server/path";
import { ContentMode, ModelProvider } from "@/lib/types/enums";

const Mode = z.enum(ContentMode);


/// A rename moves the project's slug with its name, and the page follows it:
/// the answer is the same page at the project's new path, drawn there rather
/// than here, where the old path names no project any more. Everybody else
/// with one of the project's pages open is told, and follows it too.
export async function updateProjectNameAction(
  projectId: string,
  name: string,
): Promise<ActionResult> {
  const gate = await admitProject({ call: "project:rename", limit: 10 }, projectId);
  if (!gate.ok) return refused(gate.error);

  const checked = checkProjectName(name);
  if (!checked.ok) return refused(checked.error);
  const trimmed = checked.value;

  const was = (await fetchProject(projectId))?.slug;
  const context = await getServerContext();
  const organization = context ? activeOrganization(context)?.slug : undefined;

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/projects/${encodeURIComponent(projectId)}`,
    {
      method: "PATCH",
      headers: { ...gate.headers, "content-type": "application/json" },
      body: JSON.stringify({ name: trimmed }),
    },
  );

  if (!res || !res.ok) {
    const error = res
      ? (await extractProblem(res)).message
      : "Failed to update project name. Please try again.";
    return refused(error);
  }

  const renamed = ProjectRenamedSchema.safeParse(await res.json().catch(() => null));
  if (renamed.success && was !== undefined && renamed.data.slug !== was) {
    if (organization !== undefined) {
      await publishEvent(organizationChannel(gate.organizationId), {
        type: "slug:moved",
        data: {
          organizationId: gate.organizationId,
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
/// Mail the organization's owner the code a move out of `off` spends; the
/// server sends it to the owner alone, and says so to anybody else.
export async function requestContentModeCodeAction(projectId: string): Promise<ActionResult> {
  const gate = await admitProject(
    { call: "project:content-mode-code", limit: 3, per: "hour" },
    projectId,
  );
  if (!gate.ok) return refused(gate.error);
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/telemetry/content-mode/code`,
    { method: "POST", headers: gate.headers },
  );
  if (!res) return refused("Failed to send the code. Please try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  return { ok: true };
}

/// Switch the project's mode: out of `off` with the code the owner was
/// mailed, back to `off` deleting what it kept where `deleteContent` asks.
export async function updateContentModeAction(
  projectId: string,
  mode: ContentMode,
  { code, deleteContent = false }: { code?: string; deleteContent?: boolean } = {},
): Promise<ActionResult> {
  const gate = await admitProject({ call: "project:content-mode", limit: 10 }, projectId);
  if (!gate.ok) return refused(gate.error);
  // A Server Action takes whatever a caller posts, whatever its type says.
  const chosen = Mode.safeParse(mode);
  if (!chosen.success) return refused("That is not a content mode.");
  if (code !== undefined && !isConfirmationCode(code)) return refused(NOT_A_CODE);

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/content-mode`, {
    method: "PUT",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify({
      content_mode: chosen.data,
      ...(code !== undefined && { code }),
      ...(deleteContent === true && { delete_content: true }),
    }),
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
  const gate = await admitProject({ call: "project:delete", limit: 3 }, projectId);
  if (!gate.ok) return refused(gate.error);

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/projects/${encodeURIComponent(projectId)}`,
    { method: "DELETE", headers: gate.headers },
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
  await publishEvent(organizationChannel(gate.organizationId), {
    type: "membership:removed",
    data: { organizationId: gate.organizationId, projectId },
  });

  // Every console page lists the organization's projects in its header's
  // switcher, so each one visited is read again; this one, the deleted
  // project's own, is left at once for the console's entry page, whose page
  // comes with this answer — `replace`, so Back does not return to it.
  revalidatePath("/", "layout");
  redirect("/console", RedirectType.replace);
}

/// The project's own model prices replaced whole, as its Settings section
/// holds them; the server holds each to its rules and records the change.
export async function saveModelPricesAction(
  projectId: string,
  input: unknown,
): Promise<ActionResult> {
  const gate = await admitProject({ call: "project:prices", limit: 10 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const asked = ModelPricesSchema.safeParse(input);
  if (!asked.success) return refused("Those prices could not be read.");

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/prices`, {
    method: "PUT",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify(asked.data),
  });
  if (!res) return refused("The prices could not be saved. Try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);

  revalidatePath("/(app)/[organization]/[project]/settings", "page");
  return { ok: true };
}

/** What a model connection's save sends, before the server checks its every rule. */
const ConnectionInputSchema = z.object({
  id: z.guid().optional(),
  name: z.string().min(1).max(60),
  provider: z.enum(ModelProvider),
  base_url: z.string().max(2048).optional(),
  region: z.string().max(32).optional(),
  models: z.array(z.string().max(128)).min(1).max(20),
  is_default: z.boolean(),
  secret: z.string().max(16_384).optional(),
});

/** A model connection made or changed, proved by one small request first: the provider's refusal is the answer. */
export async function saveModelConnectionAction(projectId: string, input: unknown): Promise<ActionResult> {
  const gate = await admitProject({ call: "project:model-connections", limit: 10 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const asked = ConnectionInputSchema.safeParse(input);
  if (!asked.success) return refused("That connection could not be read.");
  // A request to the provider stands in front of the save: a minute at most.
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/telemetry/model-connections`,
    { method: "PUT", headers: { ...gate.headers, "content-type": "application/json" }, body: JSON.stringify(asked.data) },
    75_000,
  );
  if (!res) return refused("The connection could not be saved. Try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  revalidatePath("/(app)/[organization]/[project]/settings", "page");
  return { ok: true };
}

/** A model connection deleted, its key with it. */
export async function deleteModelConnectionAction(projectId: string, id: string): Promise<ActionResult> {
  if (!z.guid().safeParse(id).success) return refused("Invalid connection id.");
  const gate = await admitProject({ call: "project:model-connections", limit: 10 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/model-connections/${id}`, {
    method: "DELETE",
    headers: gate.headers,
  });
  if (!res) return refused("The connection could not be deleted. Try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  revalidatePath("/(app)/[organization]/[project]/settings", "page");
  return { ok: true };
}
