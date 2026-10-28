import { notFound } from "next/navigation";

import { AccessDenied } from "@/components/access-denied";
import { MemberCeilingNotice } from "@/components/member-ceiling";
import { Invited, RoleCards, RosterTable, type RoleCard } from "@/components/members/roster";
import { MemberRow } from "@/components/members/rows";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { ownerContact } from "@/lib/identity";
import { administers, owns } from "@/lib/roles";
import {
  activeOrganization,
  fetchOrganizationInvites,
  fetchOrganizationMembers,
  getServerContext,
  identityContext,
} from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import { organizationPath } from "@/lib/slug";
import type { Role } from "@/lib/types/enums";

import { AddOrganizationMember } from "./_manage";
import {
  cancelOwnershipOfferAction,
  offerOwnershipAction,
  removeOrganizationMemberAction,
  revokeOrganizationInviteAction,
  updateOrganizationMemberRoleAction,
} from "./actions";

export const metadata = { title: "Members" };

const ROLES = {
  owner: {
    badge: "Full access",
    body: "Create and delete projects, organization-wide settings, and rolled-up audit logs.",
  },
  admin: {
    badge: "Can manage",
    body: "Create projects, manage the organization's members and settings, and work as an admin in every project.",
  },
  member: {
    badge: "Project scoped",
    body: "No organization-wide powers by default; access comes entirely from project-level roles.",
  },
} satisfies Record<Role, RoleCard>;

export default async function OrganizationMembersPage() {
  const session = await getServerSession();
  if (!session) notFound();

  // One source for both what the page shows and what its actions send:
  // `identityContext` is `/me`'s active organization — the one the path names
  // — and every action is bound to its id and acts on that one or on nothing:
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
  if (!organization || !ident || !administers(ident.role)) {
    return (
      <AccessDenied
        what="this organization's roster"
        owner={ownerContact(organization)}
        backHref={organization ? organizationPath(organization.slug) : "/console"}
      />
    );
  }

  const organizationId = organization.organizationId;
  const isOwner = owns(ident.role);

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

  // At the plan's ceiling an invitation would be refused, so the button goes
  // and the deployment's sentence stands in its place: a refusal here is a
  // stale page, never a button that cannot work.
  const full = listing.ceiling?.reached ?? false;
  const act = {
    setRole: updateOrganizationMemberRoleAction.bind(null, organizationId),
    remove: removeOrganizationMemberAction.bind(null, organizationId),
    offer: offerOwnershipAction.bind(null, organizationId),
    withdraw: cancelOwnershipOfferAction.bind(null, organizationId),
  };

  return (
    <>
      <PageHeader
        title="Members"
        action={full ? undefined : <AddOrganizationMember organizationId={organizationId} />}
      />

      <div className="grid gap-6">
        <MemberCeilingNotice ceiling={listing.ceiling} />
        <RoleCards cards={ROLES} />

        <section className="grid gap-3">
          <RosterTable>
            {listing.members.map((m) => (
              <MemberRow
                key={m.id}
                of="organization"
                member={m}
                act={act}
                isOwnerCaller={isOwner}
              />
            ))}
          </RosterTable>
          <p className="text-xs text-muted-foreground">
            Organization roles grant organization-wide capabilities. Owners manage members,
            settings, and projects. Admins create projects, manage settings and members, and
            work as admins in every project. Members receive access via individual projects.
          </p>
        </section>

        {invited.kind === "ok" && (
          <Invited
            of="organization"
            invites={invited.invites}
            revoke={revokeOrganizationInviteAction.bind(null, organizationId)}
          />
        )}
      </div>
    </>
  );
}
