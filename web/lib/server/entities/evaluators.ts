import { cache } from "react";
import { z } from "zod";

import { ScoreType } from "@/lib/types/enums";

import { valueOf } from "./read";
import { readLane } from "./traces";

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
