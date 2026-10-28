import type { NextRequest } from "next/server";

import { privateJson, privateProblem } from "@/lib/api/respond";
import { admitAgentCall } from "@/lib/server/agent-admit";
import { agentStatus } from "@/lib/server/entities/agent";

// Whether the agent can answer on a project, asked each time its window opens.
// A read, so a Route Handler: Server Actions are for writes, and the client
// sends them one at a time, so the window opened during a slow save would
// wait on the save.
export async function GET(request: NextRequest) {
  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  const gate = await admitAgentCall("agent:status", 30, projectId);
  if (!gate.ok) return privateProblem(gate.status, gate.error, gate.retryAfter);
  return privateJson({ status: await agentStatus(projectId) });
}
