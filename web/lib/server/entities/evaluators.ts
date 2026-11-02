import { cache } from "react";
import { z } from "zod";

import { EvaluationStatus, EvaluationTarget, ScoreType, SpanKind } from "@/lib/types/enums";

import { valueOf } from "./read";
import { ViewQuerySchema, readLane } from "./traces";

// An evaluator as the lanes answer it: its prompt, the score it gives with
// each end or category described for the model, and the model it judges with.
export const PromptRoleSchema = z.enum(["system", "user", "assistant"]);
export type PromptRole = z.infer<typeof PromptRoleSchema>;

export const PromptMessageSchema = z.object({ role: PromptRoleSchema, content: z.string() });
export type PromptMessage = z.infer<typeof PromptMessageSchema>;

export const ScoreSpecSchema = z.object({
  name: z.string(),
  data_type: z.enum(ScoreType),
  range_min: z.number().nullable(),
  range_max: z.number().nullable(),
  low_means: z.string(),
  high_means: z.string(),
  categories: z.array(z.object({ name: z.string(), means: z.string() })),
  multiple: z.boolean(),
});
export type ScoreSpec = z.infer<typeof ScoreSpecSchema>;

export const EvaluatorSchema = z.object({
  id: z.string(),
  version: z.number().int(),
  name: z.string(),
  kind: z.string(),
  messages: z.array(PromptMessageSchema),
  score: ScoreSpecSchema,
  reasoning: z.boolean(),
  connection_id: z.string().nullable(),
  model: z.string().nullable(),
  temperature: z.number().nullable(),
  weekly_limit: z.number().nullable(),
  created_by: z.string(),
  created_at: z.string(),
  versions: z.number().int().optional(),
});
export type Evaluator = z.infer<typeof EvaluatorSchema>;

export const EvaluatorsSchema = z.object({
  evaluators: z.array(EvaluatorSchema),
  content_kept: z.boolean(),
});
export type Evaluators = z.infer<typeof EvaluatorsSchema>;

export const EvaluatorTemplateSchema = z.object({
  key: z.string(),
  group: z.string(),
  name: z.string(),
  description: z.string(),
  messages: z.array(PromptMessageSchema),
  score: ScoreSpecSchema,
  reasoning: z.boolean(),
});
export type EvaluatorTemplate = z.infer<typeof EvaluatorTemplateSchema>;

const TemplatesSchema = z.object({ templates: z.array(EvaluatorTemplateSchema) });
const VersionsSchema = z.object({ versions: z.array(EvaluatorSchema) });

// A try's answer: the prompt as sent, and the score it gave or why it gave none.
export const TriedSchema = z.object({
  system: z.string(),
  messages: z.array(PromptMessageSchema),
  model: z.string(),
  judgement: z
    .object({
      name: z.string(),
      data_type: z.enum(ScoreType),
      value: z.number().nullable(),
      categories: z.array(z.string()),
    })
    .nullable(),
  reasoning: z.string().nullable(),
  refused: z.string().nullable(),
  input_tokens: z.number(),
  output_tokens: z.number(),
});
export type Tried = z.infer<typeof TriedSchema>;

// `null` is an outage, or an answer this console cannot read: every role on
// the project reads its evaluators.
export const fetchEvaluators = cache(
  async (projectId: string): Promise<Evaluators | null> =>
    valueOf(await readLane("fetchEvaluators", projectId, "/evaluators", EvaluatorsSchema)) ?? null,
);

export const fetchEvaluatorTemplates = cache(
  async (projectId: string): Promise<EvaluatorTemplate[] | null> =>
    valueOf(await readLane("fetchEvaluatorTemplates", projectId, "/evaluators/templates", TemplatesSchema))
      ?.templates ?? null,
);

export const fetchEvaluatorVersions = cache(
  async (projectId: string, id: string): Promise<Evaluator[] | null> =>
    z.guid().safeParse(id).success
      ? (valueOf(await readLane("fetchEvaluatorVersions", projectId, `/evaluators/${id}`, VersionsSchema))
          ?.versions ?? null)
      : null,
);

// A rule: what its evaluators judge, under the trace list's own filters,
// and the share of it they judge.
export const EvaluationRuleSchema = z.object({
  id: z.string(),
  name: z.string(),
  target: z.enum(EvaluationTarget),
  span_kind: z.enum(SpanKind).nullable(),
  span_name: z.string().nullable(),
  query: ViewQuerySchema,
  sample_rate: z.number(),
  pointers: z.record(z.string(), z.string()),
  enabled: z.boolean(),
  backfill_from: z.string().nullable(),
  backfill_seen: z.number().int(),
  backfill_done_at: z.string().nullable(),
  evaluators: z.array(z.string()),
  created_by: z.string(),
  created_at: z.string(),
  updated_at: z.string(),
});
export type EvaluationRule = z.infer<typeof EvaluationRuleSchema>;

export const EvaluationRulesSchema = z.object({
  rules: z.array(EvaluationRuleSchema),
  session_idle_minutes: z.number().int(),
});
export type EvaluationRules = z.infer<typeof EvaluationRulesSchema>;

export const RulePreviewSchema = z.object({
  days: z.number().int(),
  targets: z.number(),
  judged: z.number(),
  evaluators: z.array(
    z.object({ id: z.string(), name: z.string(), model: z.string(), cost: z.number().nullable() }),
  ),
});
export type RulePreview = z.infer<typeof RulePreviewSchema>;

export const fetchEvaluationRules = cache(
  async (projectId: string): Promise<EvaluationRules | null> =>
    valueOf(await readLane("fetchEvaluationRules", projectId, "/evaluation-rules", EvaluationRulesSchema)) ?? null,
);

// One evaluation: what it judged, where it stands, what it read and spent.
export const EvaluationSchema = z.object({
  id: z.string(),
  evaluator_version: z.number().int(),
  rule_id: z.string().nullable(),
  target: z.enum(EvaluationTarget),
  run_id: z.string().nullable(),
  span_id: z.string().nullable(),
  session_id: z.string().nullable(),
  status: z.enum(EvaluationStatus),
  attempts: z.number().int(),
  reason: z.string().nullable(),
  provider: z.string().nullable(),
  model: z.string().nullable(),
  read_at: z.string().nullable(),
  cut: z.boolean(),
  score_id: z.string().nullable(),
  trace_run_id: z.string().nullable(),
  input_tokens: z.number().nullable(),
  output_tokens: z.number().nullable(),
  cost: z.number().nullable(),
  created_at: z.string(),
  settled_at: z.string().nullable(),
});
export type Evaluation = z.infer<typeof EvaluationSchema>;

export const EvaluationsSchema = z.object({
  evaluations: z.array(EvaluationSchema),
  pending: z.number(),
  delayed: z.number(),
  done: z.number(),
  failed: z.number(),
  spent_this_week: z.number(),
});
export type Evaluations = z.infer<typeof EvaluationsSchema>;

export const fetchEvaluations = cache(
  async (projectId: string, id: string): Promise<Evaluations | null> =>
    z.guid().safeParse(id).success
      ? (valueOf(await readLane("fetchEvaluations", projectId, `/evaluators/${id}/evaluations`, EvaluationsSchema)) ??
        null)
      : null,
);
