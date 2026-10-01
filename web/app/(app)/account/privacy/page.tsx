import { notFound } from "next/navigation";

import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { Card } from "@/components/ui/card";
import { ownedOrganizationLabels } from "@/lib/identity";
import { fetchActiveSessions, getServerContext } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

import { AnalyticsPreference } from "./_analytics";
import { DeleteAccountForm } from "./_delete-account";
import { DeletedOrganizations } from "./_deleted-organizations";
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
  const deletedOrganizations = context?.deletedOrganizations ?? [];

  return (
    <>
      <PageHeader title="Privacy" />
      <div className="grid gap-6">
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

        <Section
          title="Active sessions"
          description="Every browser you are signed in on. Ending one signs that device out within a few minutes."
        >
          <ActiveSessions sessions={sessions} currentId={session.sessionRowId} />
        </Section>

        {deletedOrganizations.length > 0 && (
          <Section
            title="Deleted organizations"
            description="Organizations you own that are closed and waiting to be erased. One you deleted yourself can be restored for 14 days, whole."
          >
            <DeletedOrganizations
              organizations={deletedOrganizations}
              ownerEmail={session.email}
            />
          </Section>
        )}

        <Section
          title="Danger zone"
          danger
          description="Permanently deletes your account, and every organization you own on your own. Access ends at once and the erasure follows, usually within the hour; an account has no restore window. To delete one organization and keep your account, use that organization's settings."
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
