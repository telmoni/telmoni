import { cache } from "react";
import { z } from "zod";

import { SpanStatus } from "@/lib/types/enums";

import { ChangeSchema } from "./changes";
import type { Read } from "./read";
import { readLane } from "./traces";

// One agent over the days read: its traces, its last, and what that cost
// against the median of what its ended traces cost, none before one has ended.
const AgentSchema = z.object({
  agent: z.string(),
  traces: z.number().int(),
  failed_traces: z.number().int(),
  last_trace_id: z.string(),
  last_started_at: z.string(),
  last_ended: z.boolean(),
  last_status: z.enum(SpanStatus),
  last_spend: z.number(),
  usual_spend: z.number().nullable(),
});

export const AgentListSchema = z.object({
  days: z.number().int(),
  agents: z.array(AgentSchema),
  /** Each listed agent's newest change of each kind over the days read. */
  changes: z.array(ChangeSchema),
});
export type AgentList = z.infer<typeof AgentListSchema>;

/** The project's agents, newest last trace first, as its Agents page reads them. */
export const fetchAgents = cache(
  (projectId: string): Promise<Read<AgentList>> =>
    readLane("fetchAgents", projectId, "/agents", AgentListSchema),
);
