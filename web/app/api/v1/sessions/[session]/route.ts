import type { NextRequest } from "next/server";

import { privateJson, privateProblem } from "@/lib/api/respond";
import { isProjectId } from "@/lib/ids";
import { PINNED_MAX } from "@/lib/layout";
import { parseWindow } from "@/lib/list-query";
import { type Ceiling, admit } from "@/lib/server/admit";
import { fetchSession } from "@/lib/server/data";

/** Reads a person may make in a minute: one a change of the pinned spans,
 *  and far fewer than a hand makes. */
const READS: Ceiling = { call: "sessions:read", limit: 120 };

/** The longest a session's id is, in characters as the server counts them. */
const SESSION_ID_MAX = 200;
/** The longest a span's name is, in characters as the server counts them. */
const NAME_MAX = 145;

// One session's traces with the spans a layout pinned under each
// (`components/traces/session-runs.tsx`): the page reads the session with
// the pinned names it knows — a link's layout, or the project's — and the
// browser reads it again here with its own, which the server could not
// know. A read, so a Route Handler, and the project named by its id as every
// read here names it; telemetry decides whether the person may read it.
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ session: string }> },
) {
  const gate = await admit(READS);
  if (!gate.ok) return privateProblem(gate.status, gate.error, gate.retryAfter);

  const search = request.nextUrl.searchParams;
  const projectId = search.get("project") ?? "";
  if (!isProjectId(projectId)) return privateProblem(404, "Unknown project.");
  const sessionId = (await params).session.trim();
  if (!sessionId || [...sessionId].length > SESSION_ID_MAX) {
    return privateProblem(404, "No such session.");
  }
  const pinned = [...new Set(search.getAll("pinned").map((name) => name.trim()).filter(Boolean))];
  if (pinned.length > PINNED_MAX || pinned.some((name) => [...name].length > NAME_MAX)) {
    return privateProblem(400, `Up to ${PINNED_MAX} span names, each up to ${NAME_MAX} characters.`);
  }
  const query = parseWindow({
    days: search.get("days") ?? undefined,
    limit: search.get("limit") ?? undefined,
  });

  const access = await fetchSession(projectId, sessionId, query, pinned);
  switch (access.kind) {
    case "ok":
      return privateJson(access.session);
    case "missing":
      return privateProblem(404, "No such session.");
    case "forbidden":
      return privateProblem(403, "Not yours to read.");
    case "unavailable":
      return privateProblem(503, "The session could not be read.");
  }
}
