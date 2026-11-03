"use server";

import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { MAX_DAYS } from "@/lib/list-query";
import { admitAgentCall } from "@/lib/server/agent-admit";
import {
  type TraceFiltersAnswer,
  askTraceFilters,
  deleteAgentConversation,
} from "@/lib/server/entities/agent";
import { TRACE_QUESTION_MAX } from "@/lib/types/agent";

// The agent's writes: the panel's deletion of a conversation, and a sentence
// asked of the trace list, which spends a question. Its reads — the agent's
// status, the history, a conversation opened — are Route Handlers under
// `app/api/v1/agent`, as is the turn, which streams where a Server Action
// answers once.

export async function deleteAgentConversationAction(
  projectId: string,
  id: string,
): Promise<ActionResult> {
  const gate = await admitAgentCall("agent:delete", 30, projectId);
  if (!gate.ok) return refused(gate.error);
  if (!z.guid().safeParse(id).success) return refused("Unknown conversation.");
  return (await deleteAgentConversation(projectId, id))
    ? { ok: true }
    : refused("Couldn't delete that conversation — try again shortly.");
}

/** The trace list asked `sentence` over the `days` it shows: the query it
 *  opens on, which the page navigates to, and what the filters left out. */
export async function askTraceListAction(
  projectId: string,
  sentence: string,
  days: number,
): Promise<ActionResult<{ answer: TraceFiltersAnswer }>> {
  const gate = await admitAgentCall("agent:trace-filters", 20, projectId);
  if (!gate.ok) return refused(gate.error);
  const asked = sentence.trim();
  // In characters, as the server counts them.
  if (asked.length === 0 || [...asked].length > TRACE_QUESTION_MAX) {
    return refused(`Ask in up to ${TRACE_QUESTION_MAX} characters.`);
  }
  if (!Number.isInteger(days) || days < 1 || days > MAX_DAYS) return refused("Unknown window.");
  return askTraceFilters(projectId, asked, days);
}
