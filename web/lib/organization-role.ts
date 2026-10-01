export type OrganizationRole = "owner" | "admin" | "member";

/// The person's role in the organization `ident` names, from `/me`'s list.
///
/// ⚠ **A row, never an id comparison.** The owner is the person holding the
/// organization's `owner` entry, and it moves when the organization is
/// handed over.
export function organizationRoleOf(
  ctx: {
    organizations?: readonly {
      organizationId: string;
      role: OrganizationRole;
    }[];
  } | null,
  ident: { organizationId: string } | null,
): OrganizationRole | null {
  if (!ident) return null;
  // ⚠ The `?.` guards the LIST as well as `ctx`: a degraded `/me` without one
  // answers "no role" rather than a 500 on a page whose whole job is to decide
  // what somebody may see.
  return (
    ctx?.organizations?.find((o) => o.organizationId === ident.organizationId)?.role ?? null
  );
}

export function administersOrganization(role: OrganizationRole | null): boolean {
  return role === "owner" || role === "admin";
}
