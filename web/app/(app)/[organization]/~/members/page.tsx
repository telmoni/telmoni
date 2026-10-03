import { notFound } from "next/navigation";

import { AccessDenied } from "@/components/access-denied";
import { DataTable, THead, Th } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { ownerContact } from "@/lib/identity";
import {
  activeOrganization,
  fetchOrganizationInvites,
  fetchOrganizationMembers,
  getServerContext,
  identityContext,
} from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import { organizationPath } from "@/lib/slug";

import {
  AddOrganizationMember,
  OrganizationInviteRow,
  OrganizationMemberRow,
} from "./_manage";

export const metadata = { title: "Members" };

export default async function OrganizationMembersPage() {
  const session = await getServerSession();
  if (!session) notFound();

  // One source for both what the page shows and what its actions send:
  // `identityContext` is `/me`'s active organization — the one the path names
  // — and every action is handed its id and acts on that one or on nothing:
  // not on whichever one its request resolves once the page has gone stale.
  // Reading one organization and writing another is how this page once showed
  // one roster in a box that invited into a different organization.
  const [gate, ident] = await Promise.all([
    getServerContext(),
    identityContext(),
  ]);
  // No `/me` is an outage, not a refusal: said as one, as every sibling page
  // says it, rather than telling an owner they lack access.
  if (!gate) return <ServiceUnavailable />;
  // The path names the organization: not found unless auth answered with it.
  if (gate.organizationNotFound) notFound();
  const organization = activeOrganization(gate);

  // A MEMBER holds none, and the roster is not a thing they have an interest
  // in: auth refuses `fetchOrganizationMembers` with a 403, and the page
  // refuses before it asks rather than rendering a half-loaded shell.
  if (!organization || !ident || ident.role === "member") {
    return (
      <AccessDenied
        what="this organization's roster"
        owner={ownerContact(organization)}
        backHref={organization ? organizationPath(organization.slug) : "/console"}
      />
    );
  }

  const organizationId = organization.organizationId;
  const isOwner = ident.role === "owner";
  const canManage = isOwner || ident.role === "admin";
  const organizationNamed = Boolean(organization.name);

  // The roster and the outstanding invitations, loaded in parallel. If the
  // roster cannot be fetched the page cannot serve its purpose.
  const [listing, invited] = await Promise.all([
    fetchOrganizationMembers(),
    fetchOrganizationInvites(),
  ]);
  if (listing.kind === "unavailable") return <ServiceUnavailable />;
  if (listing.kind === "forbidden") {
    return (
      <AccessDenied
        what="this organization's roster"
        owner={ownerContact(organization)}
        backHref={organization ? organizationPath(organization.slug) : "/console"}
      />
    );
  }

  return (
    <>
      <PageHeader
        title="Members"
        action={
          canManage ? <AddOrganizationMember organizationId={organizationId} /> : undefined
        }
      />

      <div className="grid gap-3">
        <section className="grid gap-3 lg:grid-cols-3">
          <Card className="gap-1.5">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="font-medium text-sm">Owner</span>
              <Badge variant="secondary" className="text-xs shrink-0">
                Full access
              </Badge>
            </div>
            <p className="text-xs text-muted-foreground">
              Create and delete projects, organization-wide settings, and rolled-up
              audit logs.
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
              Create projects, manage the organization&apos;s members and settings, and
              work as an admin in every project.
            </p>
          </Card>
          <Card className="gap-1.5">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="font-medium text-sm">Member</span>
              <Badge variant="secondary" className="text-xs shrink-0">
                Project scoped
              </Badge>
            </div>
            <p className="text-xs text-muted-foreground">
              No organization-wide powers by default; access comes entirely from
              project-level roles.
            </p>
          </Card>
        </section>

        <section className="grid gap-3">
          {/* Fixed columns, as on the project roster: an admin's row grows a
              Transfer button, which re-flowed every column when a role
              changed. */}
          <DataTable className="table-fixed min-w-3xl">
            <THead>
              <Th>Member</Th>
              <Th className="w-40">Role</Th>
              <Th className="w-32">Added</Th>
              <Th className="w-72" />
            </THead>
            <tbody>
              {listing.members.map((m) => (
                <OrganizationMemberRow
                  key={m.id}
                  organizationId={organizationId}
                  organizationNamed={organizationNamed}
                  settingsHref={organizationPath(organization.slug, "/settings")}
                  member={m}
                  canManage={canManage}
                  isOwnerCaller={isOwner}
                />
              ))}
            </tbody>
          </DataTable>
          <p className="text-xs text-muted-foreground">
            Organization roles grant organization-wide capabilities. Owners manage members,
            settings, and projects. Admins create projects, manage settings and members, and
            work as admins in every project. Members receive access via individual projects.
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
                  <OrganizationInviteRow
                    key={i.id}
                    organizationId={organizationId}
                    invite={i}
                    canManage={canManage}
                  />
                ))}
              </tbody>
            </DataTable>
            <p className="text-xs text-muted-foreground">
              An invitation is an offer, not access — nobody on this list has
              access to your organization. Each link works once, only for the
              address it was sent to, and expires on its own.
            </p>
          </section>
        )}
      </div>
    </>
  );
}
