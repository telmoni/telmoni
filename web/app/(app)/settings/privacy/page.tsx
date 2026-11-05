import { notFound } from "next/navigation";

import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { Card } from "@/components/ui/card";
import { ownedOrganizationCount } from "@/lib/identity";
import { fetchActiveSessions, getServerContext } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

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

  const ownedOrganizations = ownedOrganizationCount(context?.organizations ?? []);

  return (
    <>
      <PageHeader title="Privacy" />
      <div className="grid gap-page">
        <Section
          title="Active sessions"
          description="Every browser you are signed in on. Ending one signs that device out within a few minutes."
        >
          <ActiveSessions sessions={sessions} currentId={session.sessionRowId} />
        </Section>

        <Section
          title="Danger zone"
          danger
          description="Permanently deletes your account, and every organization you're the only member of. Access ends at once and your data is erased, usually within the hour; it can't be undone. To delete one organization and keep your account, use that organization's settings."
        >
          <Card className="grid gap-gutter bg-destructive/5 text-sm">
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
