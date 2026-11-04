"use server";

import { revalidatePath } from "next/cache";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import {
  MODEL_CONNECTION_MODEL_MAX,
  MODEL_CONNECTION_MODELS_MAX,
  MODEL_CONNECTION_NAME_MAX,
  MODEL_CONNECTION_SECRET_MAX,
} from "@/lib/evaluators";
import { admitProject } from "@/lib/server/admit";
import { ModelProvider } from "@/lib/types/enums";

/** What a model connection's save sends, before the server checks its every rule. */
const ConnectionInputSchema = z.object({
  id: z.guid().optional(),
  name: z.string().min(1).max(MODEL_CONNECTION_NAME_MAX),
  provider: z.enum(ModelProvider),
  base_url: z.string().max(2048).optional(),
  region: z.string().max(32).optional(),
  models: z.array(z.string().max(MODEL_CONNECTION_MODEL_MAX)).min(1).max(MODEL_CONNECTION_MODELS_MAX),
  is_default: z.boolean(),
  secret: z.string().max(MODEL_CONNECTION_SECRET_MAX).optional(),
});

/** A model connection made or changed, proved by one small request first: the provider's refusal is the answer. */
export async function saveModelConnectionAction(projectId: string, input: unknown): Promise<ActionResult> {
  const gate = await admitProject({ call: "project:model-connections", limit: 10 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const asked = ConnectionInputSchema.safeParse(input);
  if (!asked.success) return refused("That connection could not be read.");
  // The proof stands in front of the save: a sign-in's two exchanges and the model, a minute each at most.
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/telemetry/model-connections`,
    { method: "PUT", headers: { ...gate.headers, "content-type": "application/json" }, body: JSON.stringify(asked.data) },
    240_000,
  );
  if (!res) return refused("The provider did not answer in time. Reload the page to see whether the connection was saved.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  revalidatePath("/(app)/[organization]/[project]/models", "page");
  return { ok: true };
}

/** A model connection deleted, its key with it. */
export async function deleteModelConnectionAction(projectId: string, id: string): Promise<ActionResult> {
  if (!z.guid().safeParse(id).success) return refused("Invalid connection id.");
  const gate = await admitProject({ call: "project:model-connections", limit: 10 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/model-connections/${encodeURIComponent(id)}`, {
    method: "DELETE",
    headers: gate.headers,
  });
  if (!res) return refused("The connection could not be deleted. Try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  revalidatePath("/(app)/[organization]/[project]/models", "page");
  return { ok: true };
}
