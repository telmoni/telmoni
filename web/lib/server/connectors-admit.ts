import "server-only";

import { z } from "zod";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { fetchProjectAnywhere, identityContext, projectHeaders } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

/** Why the connectors' lanes turn a call away, in the sentence the page shows,
 *  with the status a Route Handler answers it under. */
export interface ConnectorsRefusal {
  error: string;
  status: 401 | 404 | 429;
}

/** Where an admitted call goes, and under which headers. */
export interface ConnectorsLane {
  base: string;
  headers: Record<string, string>;
  error?: undefined;
}

export const CONNECTORS_UNAVAILABLE = "Connectors are unavailable right now — try again shortly.";
export const CONNECTORS_UNEXPECTED = "Unexpected response from the connector service.";

// The three checks every connectors call makes before it spends a request: a
// live session, a ceiling, and a project the caller is on. The role is auth's
// to derive from the bearer and the service's to decide on; nothing here
// asserts it. `connectionId` is `null` for the one call that creates a row
// rather than naming one. The page's writes are Server Actions
// (`app/(app)/[organization]/[project]/connectors/actions.ts`) and its one
// read a Route Handler (`app/api/v1/connectors/[connection]/deliveries`); both
// are let in here.
export async function admitConnectorsCall(
  call: string,
  limit: number,
  projectId: string,
  connectionId: string | null,
): Promise<ConnectorsLane | ConnectorsRefusal> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again.", status: 401 };
  const limited = await rateLimit(sessionKey(session, call), { limit, windowMs: 60_000 });
  if (limited) return { error: "Too many requests — slow down a moment.", status: 429 };
  if (connectionId !== null && !z.guid().safeParse(connectionId).success) {
    return { error: "Invalid connection.", status: 404 };
  }
  // The project's own organization: the Route Handler's path names none, and
  // the cookie follows whichever tab opened a page last.
  const [ctx, project] = await Promise.all([identityContext(), fetchProjectAnywhere(projectId)]);
  if (!ctx || !project) {
    return { error: "Couldn't resolve this project. Reload and try again.", status: 404 };
  }
  return {
    base: env.SERVER_URL,
    headers: projectHeaders({ ...ctx, organizationId: project.organizationId }, projectId),
  };
}
