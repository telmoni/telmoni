"use server";

import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { admitAgentCall } from "@/lib/server/agent-admit";
import { deleteAgentConversation } from "@/lib/server/entities/agent";

// The agent panel's one write. Its reads — the agent's status, the history,
// a conversation opened — are Route Handlers under `app/api/v1/agent`, as is the
// turn, which streams where a Server Action answers once.

export async function deleteAgentConversationAction(
  projectId: string,
  id: string,
): Promise<ActionResult> {
  const turned = await admitAgentCall("agent:delete", 30, projectId);
  if (turned) return refused(turned.error);
  if (!z.guid().safeParse(id).success) return refused("Unknown conversation.");
  return (await deleteAgentConversation(projectId, id))
    ? { ok: true }
    : refused("Couldn't delete that conversation — try again shortly.");
}
