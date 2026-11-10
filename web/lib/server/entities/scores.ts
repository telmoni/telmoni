import { cache } from "react";
import { z } from "zod";

import { ScoreSchema, type Score } from "@/lib/types/scores";

import { readLane } from "./traces";

// One score's shape is shared with the browser (`lib/types/scores.ts`), where
// the trace list's peek reads a trace's scores too.
export { ScoreSchema, type Score } from "@/lib/types/scores";

const ScoresSchema = z.object({ scores: z.array(ScoreSchema) });

export type ScoresAccess =
  | { kind: "ok"; scores: Score[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** A trace's scores, its spans' and, where named, its session's, newest first. */
export const fetchScores = cache(
  async (projectId: string, traceId: string, sessionId?: string): Promise<ScoresAccess> => {
    const query = new URLSearchParams({ trace_id: traceId });
    if (sessionId) query.set("session_id", sessionId);
    const read = await readLane("fetchScores", projectId, `/scores?${query}`, ScoresSchema);
    return read.kind === "ok" ? { kind: "ok", scores: read.value.scores } : read;
  },
);
