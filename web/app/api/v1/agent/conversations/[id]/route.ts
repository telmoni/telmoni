import type { NextRequest } from "next/server";
import { z } from "zod";

import { privateJson, privateProblem } from "@/lib/api/respond";
import { AGENT_UNAVAILABLE, admitAgentCall } from "@/lib/server/agent-admit";
import { openAgentConversation } from "@/lib/server/entities/agent";

// One past conversation, whole, for the panel to open. A read, so a Route
// Handler rather than a Server Action (`../../status`).
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ id: string }> },
) {
  const { id } = await params;
  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  const gate = await admitAgentCall("agent:open", 60, projectId);
  if (!gate.ok) return privateProblem(gate.status, gate.error, gate.retryAfter);
  if (!z.guid().safeParse(id).success) return privateProblem(404, "Unknown conversation.");
  const opened = await openAgentConversation(projectId, id);
  if (opened.kind === "ok") return privateJson({ conversation: opened.conversation });
  if (opened.kind === "missing") {
    return privateProblem(404, "That conversation no longer exists.");
  }
  return privateProblem(503, AGENT_UNAVAILABLE);
}
