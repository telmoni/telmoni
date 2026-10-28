import { AccessDenied } from "@/components/access-denied";
import { MemberCeilingNotice } from "@/components/member-ceiling";
import { Invited, RoleCards, RosterTable, type RoleCard } from "@/components/members/roster";
import { MemberRow } from "@/components/members/rows";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { ownerContact } from "@/lib/identity";
import { administers, owns } from "@/lib/roles";
import {
  activeOrganization,
  fetchInvites,
  fetchMembers,
  fetchOrganizationMembers,
} from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";
import { OrganizationRole, type Role } from "@/lib/types/enums";

import { AddMember, type Candidate } from "./_manage";
import {
  cancelProjectOfferAction,
  leaveProjectAction,
  offerProjectAction,
  removeMemberAction,
  revokeInviteAction,
  updateMemberRoleAction,
} from "./actions";

export const metadata = { title: "Members" };

const ROLES = {
  owner: {
    badge: "Full access",
    body: "Members and invitations, API keys, connectors, the audit log and this project's settings. The organization's owner, on every project it holds.",
  },
  admin: {
    badge: "Can manage",
    body: "Manages members, API keys, connectors and views the audit log. Every organization admin is one here.",
  },
  member: {
    badge: "Read-only",
    body: "Sees members, API keys and connectors; not the audit log.",
  },
} satisfies Record<Role, RoleCard>;

export default async function MembersPage({
  params,
}: PageProps<"/[organization]/[project]/members">) {
  const { organization, project: slug } = await params;

  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  // The organization's roster is its owner's and admins' to seat from, so
  // they alone are handed it; a project admin invites by email.
  const home = activeOrganization(gate);
  const fromRoster = home !== null && administers(home.role);
  const [listing, invited, roster] = await Promise.all([
    fetchMembers(project.id),
    fetchInvites(project.id),
    fromRoster ? fetchOrganizationMembers() : null,
  ]);
  if (listing.kind === "unavailable") return <ServiceUnavailable />;
  if (listing.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's roster"
        owner={ownerContact(home)}
        backHref={projectPath(organization, slug)}
      />
    );
  }

  const isOwner = owns(project.role);
  const canManage = administers(project.role);
  const me = gate.person.userId;
  // Who may be seated here with nothing to accept: the organization's members
  // holding no seat on this project. Its owner and admins hold every project.
  const seated = new Set(listing.members.map((m) => m.member_id));
  const candidates: Candidate[] =
    roster?.kind === "ok"
      ? roster.members
          .filter((m) => m.role === OrganizationRole.Member && !seated.has(m.member_id))
          .map((m) => ({
            memberId: m.member_id,
            email: m.email,
            displayName: m.display_name ?? null,
          }))
      : [];
  // An invitation counts against the owning organization's ceiling: at it,
  // one would be refused, so the deployment's sentence stands in its place. A
  // seat from the roster adds nobody, so it stays while there is one to give.
  const full = listing.ceiling?.reached ?? false;
  const offered = canManage && (!full || candidates.length > 0);
  // Every action bound to the project the page rendered, by id.
  const act = {
    setRole: updateMemberRoleAction.bind(null, project.id),
    remove: removeMemberAction.bind(null, project.id),
    offer: offerProjectAction.bind(null, project.id),
    withdraw: cancelProjectOfferAction.bind(null, project.id),
  };
  const self = { name: project.name, leave: leaveProjectAction.bind(null, project.id, "project") };

  return (
    <>
      <PageHeader
        title="Members"
        action={
          offered ? (
            <AddMember
              projectId={project.id}
              candidates={candidates}
              rosterUnread={fromRoster && roster?.kind !== "ok"}
              inviteClosed={full ? listing.ceiling?.detail : undefined}
            />
          ) : undefined
        }
      />

      <div className="grid gap-6">
        {canManage && <MemberCeilingNotice ceiling={listing.ceiling} />}
        <RoleCards cards={ROLES} />

        <section className="grid gap-3">
          {listing.members.length === 0 ? (
            <Card className="overflow-hidden p-0 gap-0">
              <div className="grid gap-1 p-4">
                <p className="text-sm font-medium">
                  Nobody else has access to this project.
                </p>
                <p className="text-sm text-muted-foreground">
                  {candidates.length > 0
                    ? "Add somebody from the organization, or invite somebody by email."
                    : "Invite somebody by email. They get a link, and nothing changes here until they accept it."}
                </p>
              </div>
            </Card>
          ) : (
            <RosterTable>
              {listing.members.map((m) => (
                <MemberRow
                  key={m.id}
                  of="project"
                  member={m}
                  act={act}
                  canManage={canManage}
                  isOwnerCaller={isOwner}
                  self={m.member_id === me ? self : undefined}
                />
              ))}
            </RosterTable>
          )}
          <p className="text-xs text-muted-foreground">
            A member has read-only access. An admin also manages members, keys,
            connectors, and sees the audit log. Only the owner can delete the project or hand it to another organization.
          </p>
        </section>

        {invited.kind === "ok" && (
          <Invited
            of="project"
            invites={invited.invites}
            canManage={canManage}
            revoke={revokeInviteAction.bind(null, project.id)}
          />
        )}
      </div>
    </>
  );
}
