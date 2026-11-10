import type { NextRequest } from "next/server";
import { z } from "zod";

import { privateJson, privateProblem } from "@/lib/api/respond";
import { isProjectId } from "@/lib/ids";
import { type Ceiling, admit } from "@/lib/server/admit";
import { fetchRunIncidents, fetchTrace } from "@/lib/server/data";
import { fetchScores } from "@/lib/server/entities/scores";

/** Reads a person may make in a minute: a list stepped through with `j` and
 *  `k` reads a trace a press, and a script looping here would have the server
 *  read thousands of spans a time. */
const READS: Ceiling = { call: "traces:read", limit: 120 };

// One run, for the trace list's peek (`components/traces/peek.tsx`), which
// reads it only once it is opened: its spans, the incidents whose notices
// showed it and its scores, as the run's own page reads them as it renders. A read, so a Route Handler, and the project
// named by its id as every read here names it; telemetry decides whether the
// person may read the project's traces.
export async function GET(request: NextRequest, { params }: RouteContext<"/api/v1/traces/[run]">) {
  const gate = await admit(READS);
  if (!gate.ok) return privateProblem(gate.status, gate.error, gate.retryAfter);

  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  if (!isProjectId(projectId)) return privateProblem(404, "Unknown project.");
  const { run } = await params;
  // A run is named by a UUID, and nothing else goes on to the server: any
  // UUID's shape, as ingest takes one, not only the versions Zod's `uuid`
  // admits.
  if (!z.guid().safeParse(run).success) return privateProblem(404, "No such trace.");

  const [access, incidents, scores] = await Promise.all([
    fetchTrace(projectId, run, true),
    fetchRunIncidents(projectId, run),
    fetchScores(projectId, run),
  ]);
  switch (access.kind) {
    case "ok":
      return privateJson({
        ...access.trace,
        incidents: incidents.map(({ id, rule_name, agent }) => ({ id, rule_name, agent })),
        scores: scores.kind === "ok" ? scores.scores : null,
      });
    case "missing":
      return privateProblem(404, "No such trace.");
    case "forbidden":
      return privateProblem(403, "Not yours to read.");
    case "unavailable":
      return privateProblem(503, "The trace could not be read.");
  }
}
