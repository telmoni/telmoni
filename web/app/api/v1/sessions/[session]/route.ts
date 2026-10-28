import { NextResponse, type NextRequest } from "next/server";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { problem } from "@/lib/api/respond";
import { isProjectId } from "@/lib/ids";
import { PINNED_MAX } from "@/lib/layout";
import { parseWindow } from "@/lib/list-query";
import { fetchSession } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

/** Reads a person may make in a minute: one a change of the pinned spans,
 *  and far fewer than a hand makes. */
const READS_A_MINUTE = { limit: 120, windowMs: 60_000 };

/** The longest a session's id is, in characters as the server counts them. */
const SESSION_ID_MAX = 200;
/** The longest a span's name is, in characters as the server counts them. */
const NAME_MAX = 145;

const PRIVATE = { "cache-control": "private, no-store" };

/** The session's own answer, so nothing in between keeps a copy. */
function reply(body: unknown) {
  return NextResponse.json(body, { headers: PRIVATE });
}

/** A read turned away, in the sentence the page shows. */
function refuse(sentence: string, status: number) {
  return problem(status, sentence, { headers: PRIVATE });
}

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
  const session = await getServerSession();
  if (!session) return refuse("Your session expired — sign in again.", 401);
  const limited = await rateLimit(sessionKey(session, "sessions:read"), READS_A_MINUTE);
  if (limited) return limited;

  const search = request.nextUrl.searchParams;
  const projectId = search.get("project") ?? "";
  if (!isProjectId(projectId)) return refuse("Unknown project.", 404);
  const sessionId = (await params).session.trim();
  if (!sessionId || [...sessionId].length > SESSION_ID_MAX) return refuse("No such session.", 404);
  const pinned = [...new Set(search.getAll("pinned").map((name) => name.trim()).filter(Boolean))];
  if (pinned.length > PINNED_MAX || pinned.some((name) => [...name].length > NAME_MAX)) {
    return refuse(`Up to ${PINNED_MAX} span names, each up to ${NAME_MAX} characters.`, 400);
  }
  const query = parseWindow({
    days: search.get("days") ?? undefined,
    limit: search.get("limit") ?? undefined,
  });

  const access = await fetchSession(projectId, sessionId, query, pinned);
  switch (access.kind) {
    case "ok":
      return reply(access.session);
    case "missing":
      return refuse("No such session.", 404);
    case "forbidden":
      return refuse("Not yours to read.", 403);
    case "unavailable":
      return refuse("The session could not be read.", 503);
  }
}
