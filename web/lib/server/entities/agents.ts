import { cache } from "react";
import { z } from "zod";

import { queryString } from "@/lib/list-query";
import { SpanFault, SpanStatus } from "@/lib/types/enums";

import { ChangeSchema } from "./changes";
import type { Read } from "./read";
import { readLane } from "./traces";

// One agent over the days read: its traces, those that failed and why, what
// they cost, its last, and what that cost against the median of what its
// ended traces cost, none before one has ended.
const AgentSchema = z.object({
  agent: z.string(),
  traces: z.number().int(),
  failed_traces: z.number().int(),
  /** Its traces carrying each fault, a trace under every one of its own. */
  faults: z.partialRecord(z.enum(SpanFault), z.number().int().nonnegative()),
  /** What its traces cost over the days read. */
  spend: z.number(),
  last_trace_id: z.string(),
  last_started_at: z.string(),
  last_ended: z.boolean(),
  last_status: z.enum(SpanStatus),
  last_spend: z.number(),
  usual_spend: z.number().nullable(),
  /** The models its traces called, the most traces first. */
  models: z.array(z.string()),
  /** The prompt versions its traces named, each `name:version`, the most traces first. */
  prompt_versions: z.array(z.string()),
});
export type Agent = z.infer<typeof AgentSchema>;

export const AgentListSchema = z.object({
  days: z.number().int(),
  agents: z.array(AgentSchema),
  /** Each listed agent's newest release and first tool call over the days read. */
  changes: z.array(ChangeSchema),
});
export type AgentList = z.infer<typeof AgentListSchema>;

/** The project's agents, newest last trace first, as its Agents page reads
 *  them; or the one `agent` names, over `days`, as its own page does. Each
 *  left out is the server's to choose: every agent, over the Agents page's days. */
export const fetchAgents = cache(
  (projectId: string, agent?: string, days?: number): Promise<Read<AgentList>> =>
    readLane("fetchAgents", projectId, `/agents${queryString({ agent, days })}`, AgentListSchema),
);
