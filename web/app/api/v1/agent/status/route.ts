import type { NextRequest } from "next/server";

import { admitAgentCall, agentRefusal, agentReply } from "@/lib/server/agent-admit";
import { agentStatus } from "@/lib/server/entities/agent";

// Whether the agent can answer on a project, asked each time its window opens.
// A read, so a Route Handler: Server Actions are for writes, and the client
// sends them one at a time, so the window opened during a slow save would
// wait on the save.
export async function GET(request: NextRequest) {
  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  const refused = await admitAgentCall("agent:status", 30, projectId);
  if (refused) return agentRefusal(refused.error, refused.status);
  return agentReply({ status: await agentStatus(projectId) });
}
