"use server";

import { revalidatePath } from "next/cache";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { admitProject } from "@/lib/server/admit";
import { PromptMessageSchema, type Tried, TriedSchema } from "@/lib/server/entities/evaluators";
import { ScoreType } from "@/lib/types/enums";

const EvaluatorInputSchema = z.object({
  id: z.guid().optional(),
  name: z.string().min(1).max(60),
  messages: z.array(PromptMessageSchema.extend({ content: z.string().max(20_000) })).min(1).max(20),
  score: z.object({
    name: z.string().min(1).max(64),
    data_type: z.enum(ScoreType),
    range_min: z.number().nullable(),
    range_max: z.number().nullable(),
    low_means: z.string().max(500),
    high_means: z.string().max(500),
    categories: z.array(z.object({ name: z.string().max(64), means: z.string().max(500) })).max(10),
    multiple: z.boolean(),
  }),
  reasoning: z.boolean(),
  connection_id: z.guid().nullable(),
  model: z.string().max(128).nullable(),
  temperature: z.number().min(0).max(2).nullable(),
});

const PAGE = "/(app)/[organization]/[project]/evaluators";
// A model's answer stands in front of a save and a try: a minute at most.
const JUDGED_MS = 75_000;

/** An evaluator saved as a new version, its model asked for the score's form first. */
export async function saveEvaluatorAction(projectId: string, input: unknown): Promise<ActionResult> {
  const gate = await admitProject({ call: "project:evaluators", limit: 20 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const asked = EvaluatorInputSchema.safeParse(input);
  if (!asked.success) return refused("That evaluator could not be read.");
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/telemetry/evaluators`,
    { method: "PUT", headers: { ...gate.headers, "content-type": "application/json" }, body: JSON.stringify(asked.data) },
    JUDGED_MS,
  );
  if (!res) return refused("The evaluator could not be saved. Try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  revalidatePath(PAGE, "page");
  return { ok: true };
}

/** A draft judged on one trace, nothing kept: the prompt as sent and what the model gave. */
export async function tryEvaluatorAction(
  projectId: string,
  input: unknown,
  runId: string,
): Promise<ActionResult<{ tried: Tried }>> {
  if (!z.guid().safeParse(runId.trim()).success) return refused("A trace is named by its run's id.");
  const gate = await admitProject({ call: "project:evaluators", limit: 20 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const asked = EvaluatorInputSchema.safeParse(input);
  if (!asked.success) return refused("That evaluator could not be read.");
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/telemetry/evaluators/try`,
    {
      method: "POST",
      headers: { ...gate.headers, "content-type": "application/json" },
      body: JSON.stringify({ evaluator: asked.data, run_id: runId.trim() }),
    },
    JUDGED_MS,
  );
  if (!res) return refused("The try did not finish. Try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  const tried = TriedSchema.safeParse(await res.json().catch(() => null));
  if (!tried.success) return refused("The try's answer could not be read.");
  return { ok: true, tried: tried.data };
}

/** An evaluator and every version deleted; the scores it gave stay. */
export async function deleteEvaluatorAction(projectId: string, id: string): Promise<ActionResult> {
  if (!z.guid().safeParse(id).success) return refused("Invalid evaluator id.");
  const gate = await admitProject({ call: "project:evaluators", limit: 20 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/evaluators/${id}`, {
    method: "DELETE",
    headers: gate.headers,
  });
  if (!res) return refused("The evaluator could not be deleted. Try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  revalidatePath(PAGE, "page");
  return { ok: true };
}
