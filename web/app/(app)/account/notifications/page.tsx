import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { getServerContext } from "@/lib/server/data";

import { IncomingInvitesSection } from "./_incoming-invites";
import { OwnershipOffersSection } from "./_ownership-offers";
import { ProjectOffersSection } from "./_project-offers";

export const metadata = { title: "Notifications" };

export default async function AccountNotificationsPage() {
  const gate = await getServerContext();
  if (!gate) return <ServiceUnavailable />;

  const invites = gate.incomingInvites ?? [];
  const offers = gate.organizations.filter((o) => o.ownershipOfferExpiresAt);
  const projectOffers = gate.projectOffers ?? [];
  // Where an accepted project may land: the organizations the person owns.
  const owned = gate.organizations.filter((o) => o.role === "owner");

  return (
    <>
      <PageHeader title="Notifications" />
      <div className="grid gap-6">
        <OwnershipOffersSection offers={offers} />
        <ProjectOffersSection offers={projectOffers} owned={owned} />
        {invites.length > 0 && <IncomingInvitesSection invites={invites} />}
        {invites.length === 0 && offers.length === 0 && projectOffers.length === 0 && (
          <Card className="overflow-hidden p-0 gap-0" data-testid="notifications-empty">
            <div className="grid gap-1 p-4">
              <p className="text-sm font-medium">No notifications</p>
              <p className="text-sm text-muted-foreground">
                Invitations to join an organization or a project, and offers
                to take one over, show up here.
              </p>
            </div>
          </Card>
        )}
      </div>
    </>
  );
}
