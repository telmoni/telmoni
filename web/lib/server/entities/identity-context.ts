import type { SessionData } from "@/lib/auth/session";
import { env } from "@/lib/env";
import type { OrganizationRole } from "@/lib/organization-role";

import { activeOrganization, getServerContext } from "./organization";
import { getServerSession } from "../session";

export interface IdentityContext {
  userId: string;
  organizationId: string;
  /// The person's role in `organizationId`, as auth answered it. For what a
  /// page SHOWS; every lane re-derives it from its own tables.
  role: OrganizationRole;
  accessToken: string;
}

/// Who is asking, and which organization they are acting in.
///
/// ⚠ **`null` when `/me` is unavailable, with no fallback.** An organization
/// is an entry resolved from auth, so there is nothing to fall back to that
/// would not risk naming the wrong tenant.
///
/// `null` too for a person in no organization at all. Nothing that acts in an
/// organization is for them; what is — their invitations, their account's
/// deletion — goes out under `personHeaders`, which names none, and their
/// account's settings under `accountHeaders`.
export async function identityContext(): Promise<IdentityContext | null> {
  const session = await getServerSession();
  if (!session) return null;
  const ctx = await getServerContext();
  if (!ctx) return null;
  const active = activeOrganization(ctx);
  if (!active) return null;

  return {
    userId: session.userId,
    organizationId: active.organizationId,
    role: active.role,
    accessToken: session.accessToken,
  };
}

// What every person lane downstream reads: the session's own bearer — an
// opaque secret auth looks up in its own tables — and the organization (and
// project) the request acts on. The console asserts nothing about WHO is
// asking — the token names the person, and a service holding it can act only
// as the one it was issued to. No role travels either: auth derives it from
// its own tables, and its siblings ask auth rather than read a header.
function bearerHeaders(
  accessToken: string,
  organizationId: string,
  projectId?: string,
): Record<string, string> {
  return {
    authorization: `Bearer ${accessToken}`,
    "x-service-secret": env.SERVICE_SECRET,
    "x-organization-id": organizationId,
    ...(projectId ? { "x-project-id": projectId } : {}),
    "x-request-id": crypto.randomUUID(),
  };
}

/// For the lanes that act on the PERSON and nothing else — their invitations
/// and their account's deletion — which must reach them whatever organization
/// they are in, including none. Auth takes no organization on these, so none
/// is sent: a stale or absent one must not be able to refuse them.
export function personHeaders(session: SessionData): Record<string, string> {
  return {
    authorization: `Bearer ${session.accessToken}`,
    "x-service-secret": env.SERVICE_SECRET,
    "x-request-id": crypto.randomUUID(),
  };
}

/// For the person's own account — their password, address, analytics consent
/// and sessions — which is theirs whatever organization they are in,
/// including none. Auth records the changes on the active organization's
/// chain, so that one is named when there is one; somebody in no organization
/// names none, and auth refuses a missing header from anybody in one.
///
/// `null` only when `/me` is unavailable: without it the console cannot tell
/// "in no organization" from "could not ask", and guessing the first would
/// send a member's change without the header auth needs to record it.
export async function accountHeaders(): Promise<Record<string, string> | null> {
  const session = await getServerSession();
  if (!session) return null;
  const ctx = await getServerContext();
  if (!ctx) return null;
  const active = activeOrganization(ctx);
  return {
    ...personHeaders(session),
    ...(active ? { "x-organization-id": active.organizationId } : {}),
  };
}

// For the lanes that act from the session directly with an organization
// resolved by hand — a sign-in step, or an action that names its target.
export function sessionHeaders(
  session: SessionData,
  organizationId: string,
  projectId?: string,
): Record<string, string> {
  return bearerHeaders(session.accessToken, organizationId, projectId);
}

export function organizationHeaders(ctx: IdentityContext): Record<string, string> {
  return bearerHeaders(ctx.accessToken, ctx.organizationId);
}

export function projectHeaders(ctx: IdentityContext, projectId: string): Record<string, string> {
  return bearerHeaders(ctx.accessToken, ctx.organizationId, projectId);
}
