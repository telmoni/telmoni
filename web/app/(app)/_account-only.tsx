import { PaperShell } from "@/components/paper-shell";
import { Section } from "@/components/section";
import { SessionHeartbeat } from "@/components/session-heartbeat";
import { SignOutButton } from "@/components/sign-out-button";
import { Card } from "@/components/ui/card";
import type { ActiveSession } from "@/lib/server/data";
import { canChangeEmail, canResetPassword } from "@/lib/sign-in-method";
import type { IncomingInvite } from "@/lib/types/incoming-invite";

import { DeleteAccountForm } from "./settings/privacy/_delete-account";
import { ActiveSessions } from "./settings/privacy/_sessions";
import { ChangeEmail } from "./settings/account/_email";
import { PasswordReset } from "./settings/account/_password";
import { IncomingInvitesSection } from "./settings/notifications/_incoming-invites";

export function AccountOnly({
  email,
  authMethod = null,
  signupsOpen,
  invites,
  ownedOrganizations,
  sessions = null,
  currentSessionId = null,
  expiresAt,
}: {
  email: string;
  authMethod?: string | null;
  signupsOpen: boolean;
  invites: readonly IncomingInvite[];
  ownedOrganizations: number;
  sessions?: ActiveSession[] | null;
  currentSessionId?: string | null;
  expiresAt: number;
}) {
  return (
    <PaperShell signedIn>
      <SessionHeartbeat expiresAt={expiresAt} />
      <main className="mx-auto grid w-full max-w-xl gap-page px-gutter py-10">
        <Card data-testid="account-only">
          <h1 className="text-base font-medium">You&apos;re not in an organization</h1>
          <p className="mt-2 text-sm text-muted-foreground">
            {signupsOpen ? (
              <>
                Reload this page to start a new organization of your own, or
                accept an invitation to join someone else&rsquo;s.
              </>
            ) : (
              <>
                Sign-ups are closed right now, so Telmoni can&rsquo;t start an
                organization for {email}. When someone invites you to theirs,
                the invitation shows up here.
              </>
            )}
          </p>
        </Card>

        {invites.length > 0 && (
          <Card>
            <IncomingInvitesSection invites={invites} />
          </Card>
        )}

        {canChangeEmail(authMethod) && (
          <Section card title="Email">
            <ChangeEmail method={authMethod} email={email} />
          </Section>
        )}

        {canResetPassword(authMethod) && (
          <Section card title="Password">
            <PasswordReset method={authMethod} />
          </Section>
        )}

        <Section card title="Active sessions">
          <ActiveSessions sessions={sessions} currentId={currentSessionId} />
        </Section>

        <Section card danger title="Delete your account">
          <DeleteAccountForm email={email} ownedOrganizations={ownedOrganizations} />
        </Section>

        <div className="text-sm">
          <SignOutButton
            variant="link"
            className="h-auto p-0 text-sm font-normal text-foreground underline underline-offset-4"
          />
        </div>
      </main>
    </PaperShell>
  );
}
