import type { NextRequest } from "next/server";
import { z } from "zod";

import { AGENT_UNAVAILABLE, admitAgentCall, agentReply } from "@/lib/server/agent-admit";
import { openAgentConversation } from "@/lib/server/entities/agent";

// One past conversation, whole, for the panel to open. A read, so a Route
// Handler rather than a Server Action (`../../status`).
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ id: string }> },
) {
  const { id } = await params;
  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  const refused = await admitAgentCall("agent:open", 60, projectId);
  if (refused) return agentReply({ error: refused.error }, refused.status);
  if (!z.guid().safeParse(id).success) {
    return agentReply({ error: "Unknown conversation." }, 404);
  }
  const opened = await openAgentConversation(projectId, id);
  if (opened.kind === "ok") return agentReply({ conversation: opened.conversation });
  if (opened.kind === "not-found") {
    return agentReply({ error: "That conversation no longer exists." }, 404);
  }
  return agentReply({ error: AGENT_UNAVAILABLE }, 503);
}
