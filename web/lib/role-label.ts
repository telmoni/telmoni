/**
 * One spelling of a role, for display.
 *
 * ⚠ **This existed six times.** `organizationRoleLabel` in
 * `lib/types/organization-role.ts`, plus five byte-identical private copies in
 * `resource-selector.tsx`, `notifications-bell.tsx`, `organization/projects/page.tsx`,
 * `account/notifications/_incoming-invites.tsx` and `[projectId]/members/_manage.tsx`
 * — some called `roleLabel`, one called `roleWord`, one inlined at the call
 * site. Nothing was wrong with any of them, which is the point: six copies of a
 * display rule are six places for the seventh reader to change one and miss
 * five.
 *
 * Serves BOTH ladders. A project role and an organization role are spelled the
 * same (`owner` / `admin` / `member`) and are the same rendering problem — the
 * wire spelling is snake-case and a person reads title case.
 */
export function roleLabel(role: string | null | undefined): string {
  if (!role) return "—";
  return role.charAt(0).toUpperCase() + role.slice(1);
}
