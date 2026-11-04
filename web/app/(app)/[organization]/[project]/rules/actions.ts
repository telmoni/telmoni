"use server";

import { revalidatePath } from "next/cache";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, readBody, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import {
  EVALUATION_RULE_EVALUATORS_MAX,
  EVALUATION_RULE_NAME_MAX,
  EVALUATION_RULE_POINTER_MAX,
} from "@/lib/evaluators";
import { admitProject } from "@/lib/server/admit";
import { type RulePreview, RulePreviewSchema } from "@/lib/server/entities/evaluators";
import { ViewQuerySchema } from "@/lib/server/entities/traces";
import { SPAN_NAME_MAX } from "@/lib/traces";
import { EvaluationTarget, SpanKind } from "@/lib/types/enums";

const PAGE = "/(app)/[organization]/[project]/rules";

const RuleInputSchema = z.object({
  id: z.guid().optional(),
  name: z.string().min(1).max(EVALUATION_RULE_NAME_MAX),
  target: z.enum(EvaluationTarget),
  span_kind: z.enum(SpanKind).nullable(),
  span_name: z.string().max(SPAN_NAME_MAX).nullable(),
  query: ViewQuerySchema,
  sample_rate: z.number().gt(0).max(1),
  pointers: z.record(z.string(), z.string().max(EVALUATION_RULE_POINTER_MAX)),
  evaluators: z.array(z.guid()).min(1).max(EVALUATION_RULE_EVALUATORS_MAX),
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
  const preview = await readBody(res, RulePreviewSchema);
  if (!preview) return refused("The preview's answer could not be read.");
  return { ok: true, preview };
}

/** A rule deleted; its evaluators stay. */
export async function deleteRuleAction(projectId: string, id: string): Promise<ActionResult> {
  if (!z.guid().safeParse(id).success) return refused("Invalid rule id.");
  const gate = await admitProject({ call: "project:evaluation-rules", limit: 20 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/evaluation-rules/${encodeURIComponent(id)}`, {
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
