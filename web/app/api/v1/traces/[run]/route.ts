import { NextResponse, type NextRequest } from "next/server";
import { z } from "zod";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { problem } from "@/lib/api/respond";
import { isProjectId } from "@/lib/ids";
import { fetchTrace } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

/** Reads a person may make in a minute: a list stepped through with `j` and
 *  `k` reads a trace a press, and a script looping here would have the server
 *  read thousands of spans a time. */
const READS_A_MINUTE = { limit: 120, windowMs: 60_000 };

const PRIVATE = { "cache-control": "private, no-store" };

/** The session's own answer, so nothing in between keeps a copy. */
function reply(body: unknown) {
  return NextResponse.json(body, { headers: PRIVATE });
}

/** A read turned away, in the sentence the peek shows. */
function refuse(sentence: string, status: number) {
  return problem(status, sentence, { headers: PRIVATE });
}

// One trace's spans, for the trace list's peek (`components/traces/peek.tsx`),
// which reads a trace only once it is opened; the trace's own page reads the
// same fetcher as it renders. A read, so a Route Handler, and the project
// named by its id as every read here names it; telemetry decides whether the
// person may read the project's traces.
export async function GET(request: NextRequest, { params }: { params: Promise<{ run: string }> }) {
  const session = await getServerSession();
  if (!session) return refuse("Your session expired — sign in again.", 401);
  const limited = await rateLimit(sessionKey(session, "traces:read"), READS_A_MINUTE);
  if (limited) return limited;

  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  if (!isProjectId(projectId)) return refuse("Unknown project.", 404);
  const { run } = await params;
  // A run is named by a UUID, and nothing else goes on to the server: any
  // UUID's shape, as ingest takes one, not only the versions Zod's `uuid`
  // admits.
  if (!z.guid().safeParse(run).success) return refuse("No such trace.", 404);

  const access = await fetchTrace(projectId, run);
  switch (access.kind) {
    case "ok":
      return reply(access.trace);
    case "missing":
      return refuse("No such trace.", 404);
    case "forbidden":
      return refuse("Not yours to read.", 403);
    case "unavailable":
      return refuse("The trace could not be read.", 503);
  }
}
