import { PaperShell } from "@/components/paper-shell";
import { SessionHeartbeat } from "@/components/session-heartbeat";
import { Card } from "@/components/ui/card";
import { analyticsConfigured } from "@/lib/analytics";
import type { ActiveSession, DeletedOrganization } from "@/lib/server/data";
import { canChangeEmail, canResetPassword } from "@/lib/sign-in-method";
import type { IncomingInvite } from "@/lib/types/incoming-invite";

import { AnalyticsPreference } from "./account/privacy/_analytics";
import { DeleteAccountForm } from "./account/privacy/_delete-account";
import { DeletedOrganizations } from "./account/privacy/_deleted-organizations";
import { ActiveSessions } from "./account/privacy/_sessions";
import { ChangeEmail } from "./account/settings/_email";
import { PasswordReset } from "./account/settings/_password";
import { IncomingInvitesSection } from "./account/notifications/_incoming-invites";

export function AccountOnly({
  email,
  authMethod = null,
  analyticsOptIn = false,
  signupsOpen,
  invites,
  ownedOrganizations,
  deletedOrganizations = [],
  sessions = null,
  currentSessionId = null,
  expiresAt,
  needsReseal,
}: {
  email: string;
  authMethod?: string | null;
  analyticsOptIn?: boolean;
  signupsOpen: boolean;
  invites: readonly IncomingInvite[];
  ownedOrganizations: readonly string[];
  deletedOrganizations?: readonly DeletedOrganization[];
  sessions?: ActiveSession[] | null;
  currentSessionId?: string | null;
  expiresAt: number;
  needsReseal?: boolean;
}) {
  return (
    <PaperShell signedIn>
      <SessionHeartbeat expiresAt={expiresAt} needsReseal={needsReseal} />
      <main className="mx-auto grid w-full max-w-xl gap-6 px-3.5 py-10">
        <Card data-testid="account-only">
          <h1 className="text-lg font-semibold">You&apos;re not in an organization</h1>
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

        {deletedOrganizations.length > 0 && (
          <div className="grid gap-2">
            <h2 className="text-base font-medium">Deleted organizations</h2>
            <DeletedOrganizations organizations={deletedOrganizations} />
          </div>
        )}

        {canChangeEmail(authMethod) && (
          <Card className="grid gap-3 text-sm">
            <h2 className="text-base font-medium">Email</h2>
            <ChangeEmail method={authMethod} email={email} />
          </Card>
        )}

        {canResetPassword(authMethod) && (
          <Card className="grid gap-3 text-sm">
            <h2 className="text-base font-medium">Password</h2>
            <PasswordReset method={authMethod} />
          </Card>
        )}

        {/* As on the privacy page: no provider, no switch. */}
        {analyticsConfigured() && (
          <Card className="grid gap-3 text-sm">
            <h2 className="text-base font-medium">Data collection</h2>
            <AnalyticsPreference optIn={analyticsOptIn} />
          </Card>
        )}

        <div className="grid gap-2">
          <h2 className="text-base font-medium">Active sessions</h2>
          <ActiveSessions sessions={sessions} currentId={currentSessionId} />
        </div>

        <Card className="grid gap-3 bg-destructive/5 text-sm">
          <h2 className="text-base font-medium">Delete your account</h2>
          <DeleteAccountForm email={email} ownedOrganizations={ownedOrganizations} />
        </Card>

        <p className="text-sm">
          {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
          <a href="/auth/logout" className="underline underline-offset-4">
            Sign out
          </a>
        </p>
      </main>
    </PaperShell>
  );
}
