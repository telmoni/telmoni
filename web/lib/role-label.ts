import type { OrganizationRole, Role } from "@/lib/types/enums";

/**
 * One spelling of a role, for display. Serves both ladders: a project role and
 * an organization role are spelled the same (`owner` / `admin` / `member`), the
 * wire spelling is snake-case and a person reads title case.
 */
export function roleLabel(role: Role | OrganizationRole): string {
  return role.charAt(0).toUpperCase() + role.slice(1);
}
