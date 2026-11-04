import { roleLabel } from "@/lib/role-label";
import { administers } from "@/lib/roles";
import type { OrganizationEntry, ProjectOffer } from "@/lib/server/entities/organization";
import {
  isBuilding,
  isWaiting,
  type AuditExport,
  type ReadyAuditExport,
} from "@/lib/types/audit-export";
import { incidentHref } from "@/lib/alerts";
import type { FeedItem } from "@/lib/server/entities/notification-feed";
import { organizationPath, projectPath } from "@/lib/slug";
import { InviteScope, NotificationKind } from "@/lib/types/enums";
import type { GivenSeat } from "@/lib/types/given-seat";
import type { IncomingInvite } from "@/lib/types/incoming-invite";

/** What the bell holds. Waiting on the person, and counted: their
 *  invitations, the seats they were given, the organizations and projects
 *  offered them, and the finished exports of the audit log of the
 *  organization the console stands in, while they still administer it. Its
 *  exports still building only show. */
export function bellItems({
  invites,
  seats,
  projectOffers,
  organizations,
  organization,
  auditExports,
}: {
  invites: readonly IncomingInvite[];
  seats: readonly GivenSeat[];
  projectOffers: readonly ProjectOffer[];
  organizations: readonly OrganizationEntry[];
  organization: OrganizationEntry | null;
  auditExports: Readonly<Record<string, readonly AuditExport[]>>;
}): {
  offers: OrganizationEntry[];
  exportsWaiting: ReadyAuditExport[];
  exportsBuilding: AuditExport[];
  count: number;
} {
  const offers = organizations.filter((o) => o.ownershipOfferExpiresAt);
  const exportsHere =
    organization && administers(organization.role)
      ? (auditExports[organization.organizationId] ?? [])
      : [];
  const exportsWaiting = exportsHere.filter(isWaiting);
  const exportsBuilding = exportsHere.filter(isBuilding);
  const count =
    invites.length + seats.length + offers.length + projectOffers.length + exportsWaiting.length;
  return { offers, exportsWaiting, exportsBuilding, count };
}

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
// grants there.
export function inviteSubtitle(invite: IncomingInvite): string {
  const level = invite.scope === InviteScope.Organization ? "Organization" : "Project";
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

// What a seat given is, in one line: the organization its project is in, and
// the role it grants there.
export function seatSubtitle(seat: GivenSeat): string {
  return `${seat.organizationName} · ${roleLabel(seat.role)}`;
}

// Who gave the seat, as `inviterText` names a sender; one who has since
// deleted their account leaves the organization's name.
export function giverText(seat: GivenSeat): string {
  const name = seat.giverDisplayName;
  if (!seat.giverEmail) return name ?? seat.organizationName;
  return name && name !== seat.giverEmail ? name : seat.giverEmail;
}

/** Where a feed's notices were raised: an organization, by its slug, and a
 *  project of it where the feed is the project's. */
export type NoticePlace = { organization: string; project?: string };

/** The page a notice is about, by its kind: an incident's under its alert, a
 *  member's the project's Members, a connector's its Connectors, and an
 *  organization's the page it names. Any other, or one whose page its row
 *  does not name, the feed's own page, where it is read in full. */
export function noticeHref(item: FeedItem, place: NoticePlace): string {
  const { organization, project } = place;
  if (project !== undefined) {
    if (item.metadata) return incidentHref(organization, project, item.metadata.incident_id);
    if (item.kind === NotificationKind.MemberAdded || item.kind === NotificationKind.MemberLeft) {
      return projectPath(organization, project, "/members");
    }
    if (
      item.kind === NotificationKind.ConnectorConnected ||
      item.kind === NotificationKind.ConnectorDisconnected
    ) {
      return projectPath(organization, project, "/connectors");
    }
    return projectPath(organization, project, "/notifications");
  }
  return item.page ?? organizationPath(organization, "/notifications");
}
