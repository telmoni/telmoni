import { notFound } from "next/navigation";

import { AccessDenied } from "@/components/access-denied";
import { MarkRead } from "@/components/mark-read";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { ownerContact } from "@/lib/identity";
import { administers } from "@/lib/roles";
import { activeOrganization, fetchNotifications, getServerContext, identityContext } from "@/lib/server/data";
import { organizationPath } from "@/lib/slug";

import { NoticeRows } from "../../_notices";
import { markOrganizationReadAction } from "../notice-actions";

export const metadata = { title: "Notifications" };

// The organization's own notices, those that name no project — its plan, its
// payments, the day's deliveries run out — which the service answers its
// owner and admins alone, so a member is told whose they are. A project's are
// on its own page.
export default async function OrganizationNotificationsPage() {
  const [gate, ident] = await Promise.all([getServerContext(), identityContext()]);
  if (!gate) return <ServiceUnavailable />;
  // The path names the organization: not found unless auth answered with it.
  if (gate.organizationNotFound) notFound();
  const organization = activeOrganization(gate);
  const overview = organization ? organizationPath(organization.slug) : "/console";

  if (!organization || !ident || !administers(ident.role)) {
    return (
      <AccessDenied
        what="this organization's notifications"
        owner={ownerContact(organization)}
        backHref={overview}
      />
    );
  }

  const feed = await fetchNotifications();
  const newest = feed?.items[0];

  return (
    <>
      <PageHeader
        title="Notifications"
        info="What happened to the organization itself — its plan, its payments, its alerts' deliveries for the day — newest first. A project's notices are on its own Notifications page. Marking them read marks them for every owner and admin."
        action={
          feed && newest && feed.unread > 0 ? (
            <MarkRead
              unread={feed.unread}
              mark={markOrganizationReadAction.bind(null, organization.organizationId, newest.created_at)}
            />
          ) : undefined
        }
      />
      <section className="grid gap-gutter" aria-label="The organization's notices">
        <NoticeRows feed={feed} place={{ organization: organization.slug }} />
      </section>
    </>
  );
}
