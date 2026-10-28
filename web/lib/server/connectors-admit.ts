import "server-only";

import { z } from "zod";

import { env } from "@/lib/env";
import { type Turned, admit } from "@/lib/server/admit";
import { fetchProjectAnywhere, identityContext, projectHeaders } from "@/lib/server/data";

/** Where an admitted call goes, and under which headers. */
export type ConnectorsLane = { ok: true; base: string; headers: Record<string, string> };

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
): Promise<ConnectorsLane | Turned> {
  const admitted = await admit({ call, limit });
  if (!admitted.ok) return admitted;
  if (connectionId !== null && !z.guid().safeParse(connectionId).success) {
    return { ok: false, error: "Invalid connection.", status: 404 };
  }
  // The project's own organization: the Route Handler's path names none, and
  // the cookie follows whichever tab opened a page last.
  const [ctx, project] = await Promise.all([identityContext(), fetchProjectAnywhere(projectId)]);
  if (!ctx || !project) {
    return { ok: false, error: "Couldn't resolve this project. Reload and try again.", status: 404 };
  }
  return {
    ok: true,
    base: env.SERVER_URL,
    headers: projectHeaders({ ...ctx, organizationId: project.organizationId }, projectId),
  };
}
