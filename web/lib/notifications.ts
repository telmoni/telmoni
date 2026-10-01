import type { IncomingInvite } from "@/lib/types/incoming-invite";
import { roleLabel } from "@/lib/role-label";

// The bell's accessible name. A screen reader hears the count and what is
// waiting, because "Notifications" alone makes the badge — which is `aria-hidden`
// decoration — the only carrier of the number. An ownership offer waits like
// an invitation does, so it is counted and named with them.
export function bellLabel(count: number): string {
  if (count <= 0) return "Notifications";
  const noun = count === 1 ? "item" : "items";
  return `Notifications: ${count} pending ${noun}`;
}

// The badge caps rather than widening the button. Two digits already push the
// circle off the icon's corner, so anything past nine reads as "more than you
// are going to triage from a dropdown".
export function badgeCount(count: number): string | null {
  if (count <= 0) return null;
  return count > 9 ? "9+" : String(count);
}

// What the invitation is for, in one line: the level it grants and the role it
// grants there. `scope` is a free string on the wire, so anything that is not
// the organization level is treated as a project rather than echoed back.
export function inviteSubtitle(invite: IncomingInvite): string {
  const level = invite.scope === "organization" ? "Organization" : "Project";
  return `${level} · ${roleLabel(invite.role)}`;
}

// Who sent it, preferring the name and falling back to the address. The two are
// often the same string, and printing both would read as two different people.
// A sender who has since deleted their account leaves neither, and the
// invitation is the organization's.
export function inviterText(invite: IncomingInvite): string {
  const name = invite.inviterDisplayName;
  if (!invite.inviterEmail) return name ?? invite.targetName;
  return name && name !== invite.inviterEmail ? name : invite.inviterEmail;
}
