import type { Metadata } from "next";
import { notFound } from "next/navigation";

import { ServiceUnavailable } from "@/components/service-unavailable";
import { organizationLabel } from "@/lib/identity";
import { activeOrganization, fetchNotifications, getServerContext } from "@/lib/server/data";

import { Notices } from "../../_notices";
import { Overview } from "../../_overview";
import { LeaveOrganization } from "../_leave-organization";
import { MarkOrganizationRead } from "../_organization-notices";

// In a route group of its own so that whatever stands in for a page while it
// loads can wrap the overview alone: beside the organization's layout it
// would wrap the project's layout too, and stream that layout's redirect and
// not-found after a page already answered 200.

// The tab is titled as the page is, by the organization's name. The context
// is read again for it, a second call auth answers from its cache.
export async function generateMetadata(): Promise<Metadata> {
  const gate = await getServerContext();
  const organization = gate && !gate.organizationNotFound ? activeOrganization(gate) : null;
  return { title: organization ? organizationLabel(organization) : "Overview" };
}

// The organization's name and nothing else, for now: the owner cleared its
// sections (projects, members, recent activity, the hosted plan) on
// 2026-10-08 while the page is rethought. Two things stay because they have
// no other home: the organization's own notices, a failed payment among them,
// for its owner and admins when there are any; and the way out, for anybody
// but the owner. Projects are reached from the switcher and the rail.
export default async function OrganizationOverviewPage() {
  const gate = await getServerContext();
  // The path names the organization: not found unless auth answered with it.
  if (gate?.organizationNotFound) notFound();
  const organization = gate ? activeOrganization(gate) : null;
  if (!organization) return <ServiceUnavailable />;

  // The organization's own name — never the name or the address of the
  // person who holds it. See `organizationLabel`.
  const name = organizationLabel(organization);

  const isOwner = organization.role === "owner";
  const isAdmin = isOwner || organization.role === "admin";
  // The organization's own feed — the notices that name no project, such as
  // an `organization_alert` a service beside this one raises — answers its
  // owner and admins, so nobody else is sent to ask for it.
  const notices = isAdmin ? await fetchNotifications() : null;

  // Anybody but the owner may leave. The owner hands the organization to an
  // admin first — the organization cannot be left with nobody holding it.
  const canLeave = !isOwner;

  // Mark read goes up to the newest notice drawn, the first of the feed.
  const newest = notices?.items[0];
  const markRead =
    notices && newest && notices.unread > 0 ? (
      <MarkOrganizationRead
        organizationId={organization.organizationId}
        unread={notices.unread}
        before={newest.created_at}
      />
    ) : null;

  return (
    <Overview
      title={name}
      action={markRead ?? undefined}
      sections={
        <>
          {notices && notices.items.length > 0 && (
            <Notices label="Organization notices" feed={notices} />
          )}
          {canLeave && (
            <LeaveOrganization organizationId={organization.organizationId} name={name} />
          )}
        </>
      }
    />
  );
}
