import { notFound } from "next/navigation";

import { LocalTime } from "@/components/local-time";
import { PageHeader } from "@/components/page-header";
import { Row, Rows } from "@/components/rows";
import { Section } from "@/components/section";
import { SettingsRow } from "@/components/settings-row";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { type LinkRefusal, identityStartPath, linkOutcome } from "@/lib/connect";
import { organizationLabel } from "@/lib/identity";
import {
  defaultOrganization,
  fetchSlackIdentities,
  getServerContext,
} from "@/lib/server/data";
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
import { UnlinkSlack } from "./_slack";

export const metadata = { title: "Account" };

// What linking a Slack account came back with, in a sentence.
const LINK_NOTICES: Record<LinkRefusal, string> = {
  denied: "Nothing was linked — the request was cancelled at Slack's page.",
  vendor: "Slack refused the sign-in. Try again.",
  callback: "That sign-in expired or was already used. Start again.",
  elsewhere: "That Slack account is linked to another account here. Unlink it there first.",
  unavailable: "Linking is unavailable right now. Try again shortly.",
  start: "Linking couldn't be started. Try again.",
};

export default async function AccountPage({
  searchParams,
}: Partial<PageProps<"/settings/account">> = {}) {
  const session = await getServerSession();
  if (!session) notFound();

  // The one thing here that is not the session's: which organization a
  // sign-in opens in. When `/me` cannot be read, or names no organization,
  // that section is left out and the rest of the page stands.
  const ctx = await getServerContext();
  const opening = ctx ? defaultOrganization(ctx) : null;
  // The Slack accounts linked, where the service can link one: left out
  // whole otherwise, as the default organization is.
  const [slack, linked] = await Promise.all([
    fetchSlackIdentities(),
    searchParams?.then(linkOutcome) ?? null,
  ]);

  const method = signInMethodLabel(session.authMethod);

  return (
    <>
      <PageHeader title="Account" />
      <div className="grid gap-page">
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

        {/* Last: what acts as you somewhere else. The anchor is where a
            refused press in Slack sends the presser. */}
        {slack?.offered && (
          <div id="slack" className="grid">
            <Section
              title="Slack"
              description="Link your Slack account, and the buttons on an incident's notice in Slack act as you, with the role you hold on its project. Only your Slack workspace and user are kept, never your address."
            >
              {linked && (
                <p
                  role="status"
                  className={
                    linked === "linked" ? "text-sm text-brand-positive" : "text-sm text-destructive"
                  }
                >
                  {linked === "linked" ? "Your Slack account is linked." : LINK_NOTICES[linked]}
                </p>
              )}
              {slack.identities.length > 0 && (
                <Rows>
                  {slack.identities.map((identity) => {
                    const workspace = identity.team_name ?? identity.team_id;
                    return (
                      <Row
                        key={identity.team_id}
                        label={workspace}
                        hint={
                          <>
                            Linked <LocalTime iso={identity.linked_at} mode="date" />
                          </>
                        }
                      >
                        <UnlinkSlack teamId={identity.team_id} workspace={workspace} />
                      </Row>
                    );
                  })}
                </Rows>
              )}
              {/* A plain anchor, not `Link`: the target starts a sign-in, and
                  a prefetch would start one on hover. */}
              <Button asChild size="sm" variant="outline" className="w-fit">
                <a href={identityStartPath("slack")}>
                  {slack.identities.length > 0
                    ? "Link another Slack account"
                    : "Link your Slack account"}
                </a>
              </Button>
            </Section>
          </div>
        )}
      </div>
    </>
  );
}
