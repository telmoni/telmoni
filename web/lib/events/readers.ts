import type { ServerContext } from "@/lib/server/entities/organization";

/** What the person reads, as `/me` answered when their stream opened. */
type Access = Pick<ServerContext, "organizations" | "memberships">;

/** Whether the person reads `projectId`, as auth decides it (`project_role`,
 *  `crates/shared/src/rbac.rs`): every project of an organization they own
 *  or administer, and each they hold a seat on. */
export function readsProject(access: Access, organizationId: string, projectId: string): boolean {
  const role = access.organizations.find((o) => o.organizationId === organizationId)?.role;
  return (
    role === "owner" || role === "admin" || access.memberships.some((m) => m.projectId === projectId)
  );
}

/** Changes whenever what the person reads does: their stream reads it once,
 *  as it opens, so the listener opens another on a change. */
export function readingKey(access: Access | null): string {
  if (!access) return "";
  return [
    ...access.organizations.map((o) => `${o.organizationId}:${o.role}`),
    ...access.memberships.map((m) => `${m.organizationId}/${m.projectId}`),
  ]
    .sort()
    .join("\n");
}
