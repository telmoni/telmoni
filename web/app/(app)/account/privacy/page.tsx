import { notFound } from "next/navigation";

import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { Card } from "@/components/ui/card";
import { analyticsConfigured } from "@/lib/analytics";
import { ownedOrganizationLabels } from "@/lib/identity";
import { fetchActiveSessions, getServerContext } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

import { AnalyticsPreference } from "./_analytics";
import { DeleteAccountForm } from "./_delete-account";
import { ActiveSessions } from "./_sessions";

export const metadata = { title: "Privacy" };

export default async function AccountPrivacyPage() {
  const session = await getServerSession();
  if (!session) notFound();

  const [sessions, context] = await Promise.all([
    fetchActiveSessions(),
    getServerContext(),
  ]);

  const ownedOrganizations = ownedOrganizationLabels(context?.organizations ?? []);

  return (
    <>
      <PageHeader title="Privacy" />
      <div className="grid gap-6">
        {/* Only where there is a provider to send to: a deployment without one
            collects nothing, and a switch would promise otherwise. */}
        {analyticsConfigured() && (
          <Section
            title="Data collection"
            description="What leaves this platform about how you use it."
          >
            <Card className="text-sm">
              <AnalyticsPreference
                optIn={context?.person.analyticsOptIn ?? false}
              />
            </Card>
          </Section>
        )}

        <Section
          title="Active sessions"
          description="Every browser you are signed in on. Ending one signs that device out within a few minutes."
        >
          <ActiveSessions sessions={sessions} currentId={session.sessionRowId} />
        </Section>

        <Section
          title="Danger zone"
          danger
          description="Permanently deletes your account, and every organization you own on your own. Access ends at once and the erasure follows, usually within the hour, with no undo. To delete one organization and keep your account, use that organization's settings."
        >
          <Card className="grid gap-3 bg-destructive/5 text-sm">
            <DeleteAccountForm
              email={session.email}
              ownedOrganizations={ownedOrganizations}
            />
          </Card>
        </Section>
      </div>
    </>
  );
}
