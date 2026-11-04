"use server";

import { revalidatePath } from "next/cache";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, readBody, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { admitProject } from "@/lib/server/admit";
import {
  PromptMessageSchema,
  type Tried,
  TriedSchema,
} from "@/lib/server/entities/evaluators";
import {
  EVALUATOR_MEANS_MAX as MEANS,
  EVALUATOR_MESSAGE_MAX,
  EVALUATOR_MESSAGES_MAX,
  EVALUATOR_NAME_MAX,
  EVALUATOR_TEMPERATURE_MAX,
  EVALUATOR_WEEKLY_LIMIT_MAX,
  MODEL_CONNECTION_MODEL_MAX,
  TYPED_INSTRUCTIONS_MAX,
  TYPED_LEVELS_MAX,
  TYPED_OPTIONS_MAX,
  TYPED_QUESTIONS_MAX,
} from "@/lib/evaluators";
import { SCORE_CATEGORIES_MAX, SCORE_CATEGORY_MAX } from "@/lib/scores";
import { SCORE_NAME_MAX } from "@/lib/traces";
import { ScoreType } from "@/lib/types/enums";

const Common = {
  id: z.guid().optional(),
  name: z.string().min(1).max(EVALUATOR_NAME_MAX),
  connection_id: z.guid().nullable(),
  model: z.string().max(MODEL_CONNECTION_MODEL_MAX).nullable(),
  weekly_limit: z.number().positive().max(EVALUATOR_WEEKLY_LIMIT_MAX).nullable(),
};

const Asked = { name: z.string().min(1).max(SCORE_NAME_MAX), instructions: z.string().min(1).max(TYPED_INSTRUCTIONS_MAX) };

const QuestionInputSchema = z.discriminatedUnion("type", [
  z.object({ ...Asked, type: z.literal("noul"), yes_means: z.string().max(MEANS), no_means: z.string().max(MEANS) }),
  z.object({ ...Asked, type: z.literal("score"), levels: z.array(z.string().min(1).max(MEANS)).min(2).max(TYPED_LEVELS_MAX) }),
  z.object({
    ...Asked,
    type: z.literal("choice"),
    options: z
      .array(z.object({ name: z.string().min(1).max(SCORE_CATEGORY_MAX), means: z.string().max(MEANS) }))
      .min(2)
      .max(TYPED_OPTIONS_MAX),
  }),
]);

const TypedInputSchema = z.object({
  ...Common,
  kind: z.literal("typed"),
  state: z.string().min(1).max(EVALUATOR_MESSAGE_MAX),
  questions: z.array(QuestionInputSchema).min(1).max(TYPED_QUESTIONS_MAX),
});

const PromptInputSchema = z.object({
  ...Common,
  kind: z.literal("prompt"),
  messages: z
    .array(PromptMessageSchema.extend({ content: z.string().max(EVALUATOR_MESSAGE_MAX) }))
    .min(1)
    .max(EVALUATOR_MESSAGES_MAX),
  score: z.object({
    name: z.string().min(1).max(SCORE_NAME_MAX),
    data_type: z.enum(ScoreType),
    range_min: z.number().nullable(),
    range_max: z.number().nullable(),
    low_means: z.string().max(MEANS),
    high_means: z.string().max(MEANS),
    categories: z
      .array(z.object({ name: z.string().max(SCORE_CATEGORY_MAX), means: z.string().max(MEANS) }))
      .max(SCORE_CATEGORIES_MAX),
    multiple: z.boolean(),
  }),
  reasoning: z.boolean(),
  media: z.boolean(),
  temperature: z.number().min(0).max(EVALUATOR_TEMPERATURE_MAX).nullable(),
});

const EvaluatorInputSchema = z.discriminatedUnion("kind", [PromptInputSchema, TypedInputSchema]);

const PAGE = "/(app)/[organization]/[project]/evaluators";
// A model's answer stands in front of a save and a try: a sign-in's two exchanges and the model, a minute each at most.
const JUDGED_MS = 240_000;

/** An evaluator saved as a new version, its model asked for its answer's form first. */
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
  const tried = await readBody(res, TriedSchema);
  if (!tried) return refused("The try's answer could not be read.");
  return { ok: true, tried };
}

/** An evaluator and every version deleted; the scores it gave stay. */
export async function deleteEvaluatorAction(projectId: string, id: string): Promise<ActionResult> {
  if (!z.guid().safeParse(id).success) return refused("Invalid evaluator id.");
  const gate = await admitProject({ call: "project:evaluators", limit: 20 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/evaluators/${encodeURIComponent(id)}`, {
    method: "DELETE",
    headers: gate.headers,
  });
  if (!res) return refused("The evaluator could not be deleted. Try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  revalidatePath(PAGE, "page");
  return { ok: true };
}
