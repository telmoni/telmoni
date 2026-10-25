import { notFound } from "next/navigation";

import { PageHeader } from "@/components/page-header";
import { Rows } from "@/components/rows";
import { Section } from "@/components/section";
import { SettingsRow } from "@/components/settings-row";
import { Card } from "@/components/ui/card";
import { organizationLabel } from "@/lib/identity";
import { defaultOrganization, getServerContext } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import {
  canChangeEmail,
  canResetPassword,
  signInMethodLabel,
} from "@/lib/sign-in-method";
import { displayName } from "@/lib/user-display";

import { DefaultOrganization } from "./_default-organization";
import { ChangeEmail } from "./_email";
import { PasswordReset } from "./_password";

export const metadata = { title: "Settings" };

export default async function AccountSettingsPage() {
  const session = await getServerSession();
  if (!session) notFound();

  // The one thing here that is not the session's: which organization a
  // sign-in opens in. When `/me` cannot be read, or names no organization,
  // that section is left out and the rest of the page stands.
  const ctx = await getServerContext();
  const opening = ctx ? defaultOrganization(ctx) : null;

  const method = signInMethodLabel(session.authMethod);

  return (
    <>
      <PageHeader title="Settings" />
      <div className="grid gap-6">
        <Section
          title="Profile"
          description="Your name and photo are managed by your identity provider during sign-in, and sync back here the next time you sign in."
        >
          <Rows>
            <SettingsRow label="Name">{displayName(session)}</SettingsRow>
            <SettingsRow label="Email">
              {session.email}
            </SettingsRow>
            {method && (
              <SettingsRow label="Sign-in method">{method}</SettingsRow>
            )}
            <SettingsRow label="User ID" copy={session.userId}>
              {session.userId}
            </SettingsRow>
          </Rows>
        </Section>

        {/* ⚠ Email and Password are drawn only for the people they apply to — a
            password account, and for Email also a method whose provider moves
            the address (`lib/sign-in-method.ts`) — and the page decides, not the
            component: the `<Section>` has to disappear whole, since a component
            returning `null` inside it leaves a titled, empty card. Hiding is not
            a permission check: the Server Actions run `canChangeEmail`
            themselves. Email sits between Profile and Password on purpose: who
            you are, then how you get in. */}
        {canChangeEmail(session.authMethod) && (
          <Section
            title="Email"
            description="The address you sign in with, and where every notice about this account goes."
          >
            <Card className="text-sm">
              <ChangeEmail method={session.authMethod} email={session.email} />
            </Card>
          </Section>
        )}

        {canResetPassword(session.authMethod) && (
          <Section
            title="Password"
            description="We email you a one-time link, and you choose the new password on the page it opens."
          >
            <Card className="text-sm">
              <PasswordReset method={session.authMethod} />
            </Card>
          </Section>
        )}

        {/* After how you get in: where signing in opens. Keyed on the default
            auth answers, so a choice saved — or one picked for you after you
            left yours — starts the control from it. */}
        {ctx && opening && (
          <Section
            title="Default organization"
            description="The organization the console opens in when you sign in. If you leave it, the console opens in another of yours."
          >
            <Card className="text-sm">
              <DefaultOrganization
                key={opening.organizationId}
                organizations={ctx.organizations.map((o) => ({
                  organizationId: o.organizationId,
                  label: organizationLabel(o),
                }))}
                current={opening.organizationId}
              />
            </Card>
          </Section>
        )}
      </div>
    </>
  );
}
