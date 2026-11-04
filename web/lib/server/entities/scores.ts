import { cache } from "react";
import { z } from "zod";

import { ScoreSource, ScoreTarget, ScoreType } from "@/lib/types/enums";

import { readLane } from "./traces";

// One score as telemetry's lane answers it: what it judges, what it says,
// and who gave it, a member by their user id and an evaluator by its id and
// version.
export const ScoreSchema = z.object({
  score_id: z.string(),
  target: z.enum(ScoreTarget),
  run_id: z.string().optional(),
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

const ScoresSchema = z.object({ scores: z.array(ScoreSchema), more: z.boolean().optional() });

export type ScoresAccess =
  | { kind: "ok"; scores: Score[]; more: boolean }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

async function scoresAt(call: string, projectId: string, query: URLSearchParams): Promise<ScoresAccess> {
  const read = await readLane(call, projectId, `/scores?${query}`, ScoresSchema);
  return read.kind === "ok" ? { kind: "ok", scores: read.value.scores, more: read.value.more ?? false } : read;
}

/** A run's scores, its spans' and, where named, its session's, newest first. */
export const fetchScores = cache(
  async (projectId: string, runId: string, sessionId?: string): Promise<ScoresAccess> => {
    const query = new URLSearchParams({ run_id: runId });
    if (sessionId) query.set("session_id", sessionId);
    return scoresAt("fetchScores", projectId, query);
  },
);

/** The project's scores, newest first, a page at a time from 0: one name's
 *  or every name's; `more` says whether another page follows. */
export const fetchProjectScores = cache(
  async (projectId: string, name?: string, page = 0): Promise<ScoresAccess> => {
    const query = new URLSearchParams();
    if (name) query.set("name", name);
    if (page > 0) query.set("page", String(page));
    return scoresAt("fetchProjectScores", projectId, query);
  },
);
