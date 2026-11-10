import { z } from "zod";

import { ScoreSource, ScoreTarget, ScoreType } from "@/lib/types/enums";

// One score as telemetry's lane answers it: what it judges, what it says,
// and who gave it, a member by their user id.
export const ScoreSchema = z.object({
  score_id: z.string(),
  target: z.enum(ScoreTarget),
  trace_id: z.string().optional(),
  span_id: z.string().optional(),
  session_id: z.string().optional(),
  name: z.string(),
  data_type: z.enum(ScoreType),
  value: z.number().nullable().optional(),
  categories: z.array(z.string()).default([]),
  range_min: z.number().nullable().optional(),
  range_max: z.number().nullable().optional(),
  source: z.enum(ScoreSource),
  scorer: z.string().optional(),
  scorer_version: z.string().optional(),
  given_at: z.string(),
});
export type Score = z.infer<typeof ScoreSchema>;
