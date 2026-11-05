import { cache } from "react";
import { z } from "zod";

import { SpanStatus } from "@/lib/types/enums";

import type { Read } from "./read";
import { readLane } from "./traces";

// One agent over the days read: its runs, its last, and what that cost
// against the median of what its ended runs cost, none before one has ended.
const AgentSchema = z.object({
  agent: z.string(),
  runs: z.number().int(),
  failed_runs: z.number().int(),
  last_run_id: z.string(),
  last_started_at: z.string(),
  last_ended: z.boolean(),
  last_status: z.enum(SpanStatus),
  last_spend: z.number(),
  usual_spend: z.number().nullable(),
});

export const AgentListSchema = z.object({
  days: z.number().int(),
  agents: z.array(AgentSchema),
});
export type AgentList = z.infer<typeof AgentListSchema>;

/** The project's agents, newest last run first, as its Agents page reads them. */
export const fetchAgents = cache(
  (projectId: string): Promise<Read<AgentList>> =>
    readLane("fetchAgents", projectId, "/agents", AgentListSchema),
);
