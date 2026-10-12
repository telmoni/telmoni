"use server";

import { z } from "zod";

import { admitAgentCall } from "@/lib/server/agent-admit";
import { deleteAgentConversation } from "@/lib/server/entities/agent";

// The agent panel's one write. Its reads — the agent's status, the history,
// a conversation opened — are Route Handlers under `app/api/agent`, as is the
// turn, which streams where a Server Action answers once.

export async function deleteAgentConversationAction(
  projectId: string,
  id: string,
): Promise<{ error: string | null }> {
  const refused = await admitAgentCall("agent:delete", 30, projectId);
  if (refused) return { error: refused.error };
  if (!z.guid().safeParse(id).success) return { error: "Unknown conversation." };
  return (await deleteAgentConversation(projectId, id))
    ? { error: null }
    : { error: "Couldn't delete that conversation — try again shortly." };
}
