import type { NextRequest } from "next/server";

import { privateJson, privateProblem } from "@/lib/api/respond";
import { AGENT_UNAVAILABLE, admitAgentCall } from "@/lib/server/agent-admit";
import { listAgentConversations } from "@/lib/server/entities/agent";

// The person's past conversations on a project, for the panel's history. A
// read, so a Route Handler rather than a Server Action (`../status`).
export async function GET(request: NextRequest) {
  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  const gate = await admitAgentCall("agent:list", 30, projectId);
  if (!gate.ok) return privateProblem(gate.status, gate.error, gate.retryAfter);
  const conversations = await listAgentConversations(projectId);
  return conversations ? privateJson({ conversations }) : privateProblem(503, AGENT_UNAVAILABLE);
}
