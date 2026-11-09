import type { NextRequest } from "next/server";

import { privateJson, privateProblem } from "@/lib/api/respond";
import { isProjectId } from "@/lib/ids";
import { isOneOf } from "@/lib/one-of";
import { type Ceiling, admit } from "@/lib/server/admit";
import { fetchVersions } from "@/lib/server/entities/versions";
import { VERSIONS_BY } from "@/lib/versions";

/** Reads a person may make in a minute: a tab or a picker changed reads once. */
const READS: Ceiling = { call: "agent-versions:read", limit: 120 };

// One agent's versions side by side, for the Agents page's panel, which reads
// them once it is opened. The agent is a parameter, never a path segment: a
// name is any text, `..` among it. Telemetry decides whether the person may
// read the project's runs, and what an agent, an environment or a prompt may be.
export async function GET(request: NextRequest) {
  const gate = await admit(READS);
  if (!gate.ok) return privateProblem(gate.status, gate.error, gate.retryAfter);

  const asked = request.nextUrl.searchParams;
  const projectId = asked.get("project") ?? "";
  if (!isProjectId(projectId)) return privateProblem(404, "Unknown project.");
  const agent = asked.get("agent") ?? "";
  const by = asked.get("by") ?? "";
  if (agent === "" || !isOneOf(VERSIONS_BY, by)) return privateProblem(400, "No such versions.");

  const read = await fetchVersions(projectId, {
    agent,
    by,
    environment: asked.get("environment") ?? undefined,
    prompt: asked.get("prompt") ?? undefined,
  });
  switch (read.kind) {
    case "ok":
      return privateJson(read.value);
    case "refused":
      return privateProblem(400, "No such versions.");
    case "forbidden":
      return privateProblem(403, "Not yours to read.");
    case "unavailable":
      return privateProblem(503, "The versions could not be read.");
  }
}
