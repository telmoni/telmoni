import { notFound } from "next/navigation";

import { PageHeader } from "@/components/page-header";
import { Rows } from "@/components/rows";
import { Section } from "@/components/section";
import { SettingsRow } from "@/components/settings-row";
import { Card } from "@/components/ui/card";
import { getServerSession } from "@/lib/server/session";
import {
  canChangeEmail,
  canResetPassword,
  signInMethodLabel,
} from "@/lib/sign-in-method";
import { displayName } from "@/lib/user-display";

import { ChangeEmail } from "./_email";
import { PasswordReset } from "./_password";

export const metadata = { title: "Settings" };

export default async function AccountSettingsPage() {
  const session = await getServerSession();
  if (!session) notFound();

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
            <SettingsRow label="Email" mono>
              {session.email}
            </SettingsRow>
            {method && (
              <SettingsRow label="Sign-in method">{method}</SettingsRow>
            )}
            <SettingsRow label="User ID" mono copy={session.userId}>
              {session.userId}
            </SettingsRow>
          </Rows>
        </Section>

        {/* ⚠ **BOTH SECTIONS ARE ABSENT ENTIRELY FOR ANYONE WHO DOES NOT SIGN
            IN WITH AN EMAIL AND PASSWORD**. Showing heading, description and card
            for options unavailable to external provider accounts is avoided, and
            the Profile row above already names how they get in.

            THE PAGE DECIDES, NOT THE COMPONENT, because the `<Section>`
            wrapper is what has to disappear: a component returning `null`
            inside it leaves a titled, described, empty card, which reads as a
            failed load rather than an absence.

            Hiding a control is not a permission check. `requestEmailChangeAction`
            and `confirmEmailChangeAction` run `canChangeEmail` themselves, and
            they are the enforcement — a Server Action is a public endpoint and
            an unrendered form does nothing to protect it.

            Email sits between Profile and Password on purpose. Profile's
            description explains that the provider owns those fields and syncs
            them back, which is the opposite of the email story — and the
            address is the identifier the password hangs off, so the two read as
            one sequence: who you are, then how you get in. */}
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
      </div>
    </>
  );
}
