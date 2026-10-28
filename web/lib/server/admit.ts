import "server-only";

import { SESSION_EXPIRED, TOO_MANY_REQUESTS } from "@/lib/action-result";
import { rateLimitRetryAfter, sessionKey } from "@/lib/api/rate-limit";
import type { SessionData } from "@/lib/auth/session";
import { isProjectId } from "@/lib/ids";
import { administers, owns } from "@/lib/roles";
import {
  type IdentityContext,
  identityContext,
  sessionHeaders,
} from "@/lib/server/entities/identity-context";
import { SWITCHED_ORGANIZATION, unplacedOrganization } from "@/lib/server/identity";
import { getServerSession } from "@/lib/server/session";

const PER = {
  minute: { windowMs: 60_000, busy: TOO_MANY_REQUESTS },
  hour: { windowMs: 3_600_000, busy: "Too many requests. Try again in an hour." },
} as const;

/** How many of one kind of call a person may make: a minute's worth, or an
 *  hour's for a call that mails a code or spends one. */
export interface Ceiling {
  call: string;
  limit: number;
  per?: keyof typeof PER;
}

/** Why a call was turned away: the sentence the page shows, and the status a
 *  Route Handler answers it under, with when to try again after a 429. */
export type Turned = {
  ok: false;
  error: string;
  status: 401 | 403 | 404 | 409 | 429;
  retryAfter?: number;
};

export type Admitted = { ok: true; session: SessionData };

export const UNRESOLVED_PROJECT = "Couldn't resolve that project. Reload and try again.";

const turned = (status: Turned["status"], error: string): Turned => ({ ok: false, error, status });

/** A live session, under the call's ceiling. */
export async function admit({ call, limit, per = "minute" }: Ceiling): Promise<Admitted | Turned> {
  const session = await getServerSession();
  if (!session) return turned(401, SESSION_EXPIRED);
  const { windowMs, busy } = PER[per];
  const retryAfter = await rateLimitRetryAfter(sessionKey(session, call), { limit, windowMs });
  if (retryAfter !== null) return { ...turned(429, busy), retryAfter };
  return { ok: true, session };
}

/** Admitted on the project its page rendered, by id, in the organization the
 *  session stands in: auth takes the organization from the project itself, so
 *  a stale page cannot aim a write at a project it is not showing. */
export async function admitProject(
  ceiling: Ceiling,
  projectId: string,
): Promise<
  | (Admitted & { organizationId: string; projectId: string; headers: Record<string, string> })
  | Turned
> {
  const admitted = await admit(ceiling);
  if (!admitted.ok) return admitted;
  // The id becomes a header of the lane's request, and a Server Action takes
  // whatever is posted to it.
  if (!isProjectId(projectId)) return turned(404, UNRESOLVED_PROJECT);
  const ident = await identityContext();
  if (!ident) return turned(404, await unplacedOrganization());
  return {
    ...admitted,
    organizationId: ident.organizationId,
    projectId,
    headers: sessionHeaders(admitted.session, ident.organizationId, projectId),
  };
}

/** Admitted in the organization its page rendered, holding the role the call
 *  needs. The organization is checked first, so a refusal never blames a role
 *  held somewhere else. */
export async function admitOrganization(
  ceiling: Ceiling,
  organizationId: string,
  needs?: { role: "admin" | "owner"; refused: string },
): Promise<(Admitted & { ctx: IdentityContext }) | Turned> {
  const admitted = await admit(ceiling);
  if (!admitted.ok) return admitted;
  const ctx = await identityContext();
  if (!ctx) return turned(404, await unplacedOrganization());
  if (ctx.organizationId !== organizationId) return turned(409, SWITCHED_ORGANIZATION);
  if (needs && !(needs.role === "owner" ? owns(ctx.role) : administers(ctx.role))) {
    return turned(403, needs.refused);
  }
  return { ...admitted, ctx };
}
