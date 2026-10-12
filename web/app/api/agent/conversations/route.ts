import type { NextRequest } from "next/server";

import { AGENT_UNAVAILABLE, admitAgentCall, agentReply } from "@/lib/server/agent-admit";
import { listAgentConversations } from "@/lib/server/entities/agent";

// The person's past conversations on a project, for the panel's history. A
// read, so a Route Handler rather than a Server Action (`../status`).
export async function GET(request: NextRequest) {
  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  const refused = await admitAgentCall("agent:list", 30, projectId);
  if (refused) return agentReply({ error: refused.error }, refused.status);
  const conversations = await listAgentConversations(projectId);
  return conversations
    ? agentReply({ conversations })
    : agentReply({ error: AGENT_UNAVAILABLE }, 503);
}
