/**
 * One spelling of a role, for display. Serves both ladders: a project role and
 * an organization role are spelled the same (`owner` / `admin` / `member`), the
 * wire spelling is snake-case and a person reads title case.
 */
export function roleLabel(role: string | null | undefined): string {
  if (!role) return "—";
  return role.charAt(0).toUpperCase() + role.slice(1);
}
