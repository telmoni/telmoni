import "server-only";

import { NextResponse } from "next/server";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { isProjectId } from "@/lib/connect";
import { getServerSession } from "@/lib/server/session";

/** Why the agent's lanes turn a call away, in the sentence the panel shows,
 *  with the status a Route Handler answers it under. */
export interface AgentRefusal {
  error: string;
  status: 401 | 404 | 429;
}

export const AGENT_UNAVAILABLE = "The agent is unavailable right now — try again shortly.";

// Who may ask the agent anything about a project: a session, under the call's
// own ceiling a minute, naming a project id. The panel's reads are Route
// Handlers (`app/api/agent`) and its one write a Server Action
// (`app/(app)/agent-actions.ts`); both are let in here.
export async function admitAgentCall(
  call: string,
  limit: number,
  projectId: string,
): Promise<AgentRefusal | null> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again.", status: 401 };
  const limited = await rateLimit(sessionKey(session, call), { limit, windowMs: 60_000 });
  if (limited) return { error: "Too many requests — slow down a moment.", status: 429 };
  if (!isProjectId(projectId)) return { error: "Unknown project.", status: 404 };
  return null;
}

/** A read's answer: the session's own, so nothing in between keeps a copy. */
export function agentReply(body: unknown, status = 200): NextResponse {
  return NextResponse.json(body, { status, headers: { "cache-control": "private, no-store" } });
}
