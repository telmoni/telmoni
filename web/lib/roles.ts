import { type OrganizationRole, Role } from "@/lib/types/enums";

/// Admin and up, on either ladder, as `crates/shared/src/rbac.rs` has it: who
/// may change a project and read its audit log, and manage an organization.
export function administers(role: Role | OrganizationRole | null | undefined): boolean {
  return role === Role.Owner || role === Role.Admin;
}

/// The owner, on either ladder.
export function owns(role: Role | OrganizationRole | null | undefined): boolean {
  return role === Role.Owner;
}

/// What an invitation or a role change may hand out: never ownership, which
/// is handed over.
export type GivenRole = Exclude<Role, typeof Role.Owner>;

/// A Server Action takes whatever is posted to it, whatever its parameter's
/// type says, so a role is checked here before it is sent anywhere.
export function isGivenRole(role: unknown): role is GivenRole {
  return role === Role.Admin || role === Role.Member;
}

export const NOT_A_GIVEN_ROLE = "Choose admin or member.";

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
