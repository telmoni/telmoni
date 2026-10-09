import type { Metadata } from "next";
import { notFound } from "next/navigation";

import { LocalTime } from "@/components/local-time";
import { Row, Rows } from "@/components/rows";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { organizationLabel } from "@/lib/identity";
import { eventKindLabel } from "@/lib/notification-kinds";
import {
  activeOrganization,
  fetchNotifications,
  getServerContext,
  type FeedItem,
  type NotificationFeed,
} from "@/lib/server/data";
import { cn } from "@/lib/utils";

import { Overview } from "../_overview";
import { LeaveOrganization } from "./_leave-organization";
import { MarkOrganizationRead } from "./_organization-notices";

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

  const markRead =
    notices && notices.unread > 0 ? (
      <MarkOrganizationRead organizationId={organization.organizationId} unread={notices.unread} />
    ) : null;

  return (
    <Overview
      title={name}
      action={markRead ?? undefined}
      sections={
        <>
          {notices && notices.items.length > 0 && <OrganizationNotices feed={notices} />}
          {canLeave && (
            <LeaveOrganization organizationId={organization.organizationId} name={name} />
          )}
        </>
      }
    />
  );
}

function OrganizationNotices({ feed }: { feed: NotificationFeed }) {
  return (
    <section className="grid gap-3" aria-label="Organization notices">
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-sm font-medium">Organization notices</h3>
      </div>
      <Rows>
        {feed.items.map((item) => (
          <OrganizationNoticeRow key={item.id} item={item} />
        ))}
      </Rows>
    </section>
  );
}

function OrganizationNoticeRow({ item }: { item: FeedItem }) {
  const unread = item.read_at === null;
  return (
    <Row
      label={
        <span
          className={cn(
            "block min-w-0 truncate",
            unread ? "font-bold text-foreground" : "font-normal text-muted-foreground",
          )}
        >
          <span>{item.title}</span>
          {unread && <span className="sr-only"> Unread</span>}
        </span>
      }
      hint={
        <span className="block">
          <span className={cn(unread ? "text-foreground/80" : "text-muted-foreground")}>
            {item.body}
          </span>
          <span className="mt-0.5 block text-xs opacity-70">{eventKindLabel(item.kind)}</span>
        </span>
      }
    >
      <span className={cn(unread ? "font-medium text-foreground" : "text-muted-foreground")}>
        <LocalTime iso={item.created_at} mode="datetime" />
      </span>
    </Row>
  );
}
