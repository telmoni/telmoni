import type { NextRequest } from "next/server";

import { privateJson, privateProblem } from "@/lib/api/respond";
import { isProjectId } from "@/lib/ids";
import { type Ceiling, admit } from "@/lib/server/admit";
import { fetchTraceIncidents, fetchTrace } from "@/lib/server/data";
import { fetchScores } from "@/lib/server/entities/scores";
import { TraceIdSchema } from "@/lib/types/traces";

/** Reads a person may make in a minute: a list stepped through with `j` and
 *  `k` reads a trace a press, and a script looping here would have the server
 *  read thousands of spans a time. */
const READS: Ceiling = { call: "traces:read", limit: 120 };

// One trace, for the list's peek (`components/traces/peek.tsx`), which reads
// it only once it is opened: its spans, the incidents whose notices showed it
// and its scores, as the trace's own page reads them as it renders. A read,
// so a Route Handler, and the project named by its id as every read here
// names it; telemetry decides whether the person may read the project's
// traces.
export async function GET(request: NextRequest, { params }: RouteContext<"/api/v1/traces/[trace]">) {
  const gate = await admit(READS);
  if (!gate.ok) return privateProblem(gate.status, gate.error, gate.retryAfter);

  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  if (!isProjectId(projectId)) return privateProblem(404, "Unknown project.");
  const { trace } = await params;
  // A trace is named by 32 hex digits, and nothing else goes on to the server.
  if (!TraceIdSchema.safeParse(trace).success) return privateProblem(404, "No such trace.");

  const [access, incidents, scores] = await Promise.all([
    fetchTrace(projectId, trace, true),
    fetchTraceIncidents(projectId, trace),
    fetchScores(projectId, trace),
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
