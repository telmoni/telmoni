"use server";

import { revalidatePath } from "next/cache";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { admitProject } from "@/lib/server/admit";
import {
  PromptMessageSchema,
  type RulePreview,
  RulePreviewSchema,
  type Tried,
  TriedSchema,
} from "@/lib/server/entities/evaluators";
import { ViewQuerySchema } from "@/lib/server/entities/traces";
import { EvaluationTarget, ScoreType, SpanKind } from "@/lib/types/enums";

const Common = {
  id: z.guid().optional(),
  name: z.string().min(1).max(60),
  connection_id: z.guid().nullable(),
  model: z.string().max(128).nullable(),
  weekly_limit: z.number().positive().max(1_000_000).nullable(),
};

const Asked = { name: z.string().min(1).max(64), instructions: z.string().min(1).max(4_000) };

const QuestionInputSchema = z.discriminatedUnion("type", [
  z.object({ ...Asked, type: z.literal("noul"), yes_means: z.string().max(500), no_means: z.string().max(500) }),
  z.object({ ...Asked, type: z.literal("score"), levels: z.array(z.string().min(1).max(500)).min(2).max(10) }),
  z.object({
    ...Asked,
    type: z.literal("choice"),
    options: z.array(z.object({ name: z.string().min(1).max(64), means: z.string().max(500) })).min(2).max(255),
  }),
]);

const TypedInputSchema = z.object({
  ...Common,
  kind: z.literal("typed"),
  state: z.string().min(1).max(20_000),
  questions: z.array(QuestionInputSchema).min(1).max(16),
});

const PromptInputSchema = z.object({
  ...Common,
  kind: z.literal("prompt"),
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
  media: z.boolean(),
  temperature: z.number().min(0).max(2).nullable(),
});

const EvaluatorInputSchema = z.discriminatedUnion("kind", [PromptInputSchema, TypedInputSchema]);

const PAGE = "/(app)/[organization]/[project]/evaluators";
// A model's answer stands in front of a save and a try: a minute at most.
const JUDGED_MS = 75_000;

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

const RuleInputSchema = z.object({
  id: z.guid().optional(),
  name: z.string().min(1).max(60),
  target: z.enum(EvaluationTarget),
  span_kind: z.enum(SpanKind).nullable(),
  span_name: z.string().max(200).nullable(),
  query: ViewQuerySchema,
  sample_rate: z.number().gt(0).max(1),
  pointers: z.record(z.string(), z.string().max(200)),
  evaluators: z.array(z.guid()).min(1).max(10),
  enabled: z.boolean(),
  backfill_from: z.iso.datetime().nullable(),
});

/** A rule made or changed, held to the plan's number of rules. */
export async function saveRuleAction(projectId: string, input: unknown): Promise<ActionResult> {
  const gate = await admitProject({ call: "project:evaluation-rules", limit: 20 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const asked = RuleInputSchema.safeParse(input);
  if (!asked.success) return refused("That rule could not be read.");
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/evaluation-rules`, {
    method: "PUT",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify(asked.data),
  });
  if (!res) return refused("The rule could not be saved. Try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  revalidatePath(PAGE, "page");
  return { ok: true };
}

/** A draft rule against the last seven days: its targets and what judging them would cost. */
export async function previewRuleAction(
  projectId: string,
  input: unknown,
): Promise<ActionResult<{ preview: RulePreview }>> {
  const gate = await admitProject({ call: "project:evaluation-rules", limit: 20 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const asked = RuleInputSchema.safeParse(input);
  if (!asked.success) return refused("That rule could not be read.");
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/evaluation-rules/preview`, {
    method: "POST",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify(asked.data),
  });
  if (!res) return refused("The preview did not finish. Try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  const preview = RulePreviewSchema.safeParse(await res.json().catch(() => null));
  if (!preview.success) return refused("The preview's answer could not be read.");
  return { ok: true, preview: preview.data };
}

/** A rule deleted; its evaluators stay. */
export async function deleteRuleAction(projectId: string, id: string): Promise<ActionResult> {
  if (!z.guid().safeParse(id).success) return refused("Invalid rule id.");
  const gate = await admitProject({ call: "project:evaluation-rules", limit: 20 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/evaluation-rules/${id}`, {
    method: "DELETE",
    headers: gate.headers,
  });
  if (!res) return refused("The rule could not be deleted. Try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  revalidatePath(PAGE, "page");
  return { ok: true };
}

/** How long a session is quiet before a rule judges it. */
export async function saveSessionIdleAction(projectId: string, minutes: number): Promise<ActionResult> {
  if (!Number.isInteger(minutes) || minutes < 2 || minutes > 1440) {
    return refused("A session is judged after 2 to 1,440 quiet minutes.");
  }
  const gate = await admitProject({ call: "project:evaluation-rules", limit: 20 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/evaluation-rules/session-idle`, {
    method: "PUT",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify({ minutes }),
  });
  if (!res) return refused("The idle time could not be saved. Try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);
  revalidatePath(PAGE, "page");
  return { ok: true };
}
