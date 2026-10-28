import "server-only";

import { isProjectId } from "@/lib/ids";
import { type Admitted, type Turned, admit } from "@/lib/server/admit";

export const AGENT_UNAVAILABLE = "The agent is unavailable right now — try again shortly.";

// Who may ask the agent anything about a project: a session, under the call's
// own ceiling a minute, naming a project id. The panel's reads are Route
// Handlers (`app/api/v1/agent`) and its one write a Server Action
// (`app/(app)/agent-actions.ts`); both are let in here.
export async function admitAgentCall(
  call: string,
  limit: number,
  projectId: string,
): Promise<Admitted | Turned> {
  const admitted = await admit({ call, limit });
  if (!admitted.ok || isProjectId(projectId)) return admitted;
  return { ok: false, error: "Unknown project.", status: 404 };
}
