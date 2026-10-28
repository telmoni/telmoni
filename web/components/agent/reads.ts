import { z } from "zod";

import { readRoute } from "@/components/route-read";
import { refusalWords } from "@/lib/route-read";
import {
  AgentConversationSchema,
  AgentConversationSummarySchema,
  AgentStatusSchema,
} from "@/lib/types/agent";

// The agent panel's reads, from the agent's Route Handlers (`app/api/v1/agent`),
// made in the browser and so kept out of `lib/`, where a bare fetch is a
// server's. A refusal comes back as the sentence the panel shows: the
// handler's own, or — for a lapsed session the console's proxy turned away
// on `/api/*` before any handler ran — the one the handler would have given.

const UNREACHABLE = "The agent is unavailable right now — try again shortly.";

async function read<T extends object>(
  path: string,
  schema: z.ZodType<T>,
): Promise<T | { error: string }> {
  const answer = await readRoute(path, schema);
  return answer.kind === "ok" ? answer.value : { error: refusalWords(answer, UNREACHABLE) };
}

const query = (projectId: string) => `project=${encodeURIComponent(projectId)}`;

export function readAgentStatus(projectId: string) {
  return read(`/api/v1/agent/status?${query(projectId)}`, z.object({ status: AgentStatusSchema }));
}

export function readAgentConversations(projectId: string) {
  return read(
    `/api/v1/agent/conversations?${query(projectId)}`,
    z.object({ conversations: z.array(AgentConversationSummarySchema) }),
  );
}

export function readAgentConversation(projectId: string, id: string) {
  return read(
    `/api/v1/agent/conversations/${encodeURIComponent(id)}?${query(projectId)}`,
    z.object({ conversation: AgentConversationSchema }),
  );
}
