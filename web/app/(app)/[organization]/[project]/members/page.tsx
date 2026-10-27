import { DataTable, THead, Th } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { AccessDenied } from "@/components/access-denied";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { MemberCeilingNotice, atCeiling } from "@/components/member-ceiling";
import { ownerContact } from "@/lib/identity";
import { administersOrganization } from "@/lib/organization-role";
import { administersProject } from "@/lib/project-role";
import { Role } from "@/lib/types/enums";
import {
  activeOrganization,
  fetchInvites,
  fetchMembers,
  fetchOrganizationMembers,
} from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";

import { AddMember, type Candidate, InviteRow, MemberRow } from "./_manage";

export const metadata = { title: "Members" };

export default async function MembersPage({
  params,
}: {
  params: Promise<{ organization: string; project: string }>;
}) {
  const { organization, project: slug } = await params;

  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  // The organization's roster is its owner's and admins' to seat from, so
  // they alone are handed it; a project admin invites by email.
  const home = activeOrganization(gate);
  const fromRoster = home !== null && administersOrganization(home.role);
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

  const isOwner = project.role === Role.Owner;
  const canManage = administersProject(project.role);
  const me = gate.person.userId;
  // Who may be seated here with nothing to accept: the organization's members
  // holding no seat on this project. Its owner and admins hold every project.
  const seated = new Set(listing.members.map((m) => m.member_id));
  const candidates: Candidate[] =
    roster?.kind === "ok"
      ? roster.members
          .filter((m) => m.role === "member" && !seated.has(m.member_id))
          .map((m) => ({
            memberId: m.member_id,
            email: m.email,
            displayName: m.display_name ?? null,
          }))
      : [];
  // An invitation counts against the owning organization's ceiling: at it,
  // one would be refused, so the deployment's sentence stands in its place. A
  // seat from the roster adds nobody, so it stays while there is one to give.
  const full = atCeiling(listing.ceiling);
  const offered = canManage && (!full || candidates.length > 0);

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
        <section className="grid gap-3 lg:grid-cols-3">
          <Card className="gap-1.5">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="font-medium text-sm">Owner</span>
              <Badge variant="secondary" className="text-xs shrink-0">
                Full access
              </Badge>
            </div>
            <p className="text-xs text-muted-foreground">
              Members and invitations, API keys, connectors, the audit log and
              this project&apos;s settings. The organization&apos;s owner, on
              every project it holds.
            </p>
          </Card>
          <Card className="gap-1.5">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="font-medium text-sm">Admin</span>
              <Badge variant="secondary" className="text-xs shrink-0">
                Can manage
              </Badge>
            </div>
            <p className="text-xs text-muted-foreground">
              Manages members, API keys, connectors and views the audit log.
              Every organization admin is one here.
            </p>
          </Card>
          <Card className="gap-1.5">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="font-medium text-sm">Member</span>
              <Badge variant="secondary" className="text-xs shrink-0">
                Read-only
              </Badge>
            </div>
            <p className="text-xs text-muted-foreground">
              Sees members, API keys and connectors; not the audit log.
            </p>
          </Card>
        </section>

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
            // Fixed columns: an admin's row grows a Transfer button, and with
            // the widths left to the content, changing a role slid the Role
            // and Added columns under the cursor. The last column fits the
            // widest pair of buttons a row can carry.
            <DataTable className="table-fixed min-w-3xl">
              <THead>
                <Th>Member</Th>
                <Th className="w-40">Role</Th>
                <Th className="w-32">Added</Th>
                <Th className="w-72" />
              </THead>
              <tbody>
                {listing.members.map((m) => (
                  <MemberRow
                    key={m.id}
                    projectId={project.id}
                    projectName={project.name}
                    member={m}
                    canManage={canManage}
                    isOwnerCaller={isOwner}
                    isSelf={m.member_id === me}
                  />
                ))}
              </tbody>
            </DataTable>
          )}
          <p className="text-xs text-muted-foreground">
            A member has read-only access. An admin also manages members, keys,
            connectors, and sees the audit log. Only the owner can delete the project or hand it to another organization.
          </p>
        </section>

        {invited.kind === "ok" && invited.invites.length > 0 && (
          <section className="grid gap-3">
            <h2 className="text-sm font-medium">Invited</h2>
            <DataTable>
              <THead>
                <Th>Email address</Th>
                <Th>Role</Th>
                <Th>Invitation</Th>
                <Th />
              </THead>
              <tbody>
                {invited.invites.map((i) => (
                  <InviteRow
                    key={i.id}
                    projectId={project.id}
                    invite={i}
                    canManage={canManage}
                  />
                ))}
              </tbody>
            </DataTable>
            <p className="text-xs text-muted-foreground">
              An invitation is an offer, not access — nobody on this list has
              access to your project. Each link works once, only for the address it
              was sent to, and expires on its own.
            </p>
          </section>
        )}
      </div>
    </>
  );
}
