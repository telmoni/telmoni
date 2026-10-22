import { ConsoleUiProvider } from "@/components/console-ui-context";
import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import { Suspense } from "react";

import { ConsoleHeader } from "@/components/console-header";
import { ConsoleShell } from "@/components/console-shell";
import { ConsoleSidebar } from "@/components/console-sidebar";
import { CreatePanels } from "@/components/create-panels";
import { KeyboardShortcuts } from "@/components/keyboard-shortcuts";
import { SkipLink } from "@/components/skip-link";
import { NAV_COLLAPSED_COOKIE } from "@/lib/console-nav";
import { LOCAL_TIME_COOKIE, parseReader } from "@/lib/local-time";
import { PagePrimaryActionProvider } from "@/components/page-action";
import { SidebarProvider } from "@/components/sidebar-context";
import { ProjectBanner } from "@/components/project-banner";
import { ExtensionBanner } from "@/components/extension/banner";
import { ExtensionCreateOrganizationNotice } from "@/components/extension/create-organization-notice";
import { ExtensionOrganizationBadge } from "@/components/extension/organization-badge";
import { InviteToastNotifier } from "@/components/invite-toast-notifier";
import { LocalTimeReader } from "@/components/local-time";
import { OrganizationSync } from "@/components/organization-sync";
import { RealtimeListener } from "@/components/realtime-listener";
import {
  activeOrganization,
  fetchActiveSessions,
  getServerContext,
  fetchProjectAnnouncement,
} from "@/lib/server/data";
import { env } from "@/lib/env";
import { readingKey } from "@/lib/events/readers";
import { getServerSession } from "@/lib/server/session";
import { storeSeed } from "@/lib/server/store-seed";
import { VERSION } from "@/lib/server/version";
import { Flag, flagOn } from "@/lib/flags";
import { StoreProvider } from "@/lib/store/provider";
import { accountOnly } from "@/lib/account-only";
import { ownedOrganizationCount } from "@/lib/identity";
import { SessionHeartbeat } from "@/components/session-heartbeat";
import { ConsoleTrailRecorder } from "@/components/console-trail-recorder";
import { ConsoleSearch } from "@/components/console-search";

import { AccountOnly } from "./_account-only";

export default async function AppLayout({ children }: { children: React.ReactNode }) {
  const [session, ctx, seed, projectAnnouncement] = await Promise.all([
    getServerSession(),
    getServerContext(),
    storeSeed(),
    fetchProjectAnnouncement(),
  ]);

  if (!session || !seed) redirect("/auth/logout");

  const jar = await cookies();
  const reader = parseReader(jar.get(LOCAL_TIME_COOKIE)?.value);

  // The listener runs in the browser and reads no environment of its own; it
  // opens its stream again when the projects the person reads change.
  const liveHoldMs = env.LIVE_REFRESH_HOLD_SECS * 1_000;
  const reading = readingKey(ctx);

  // Every route under (app) answers this way while the gate is up, so no page
  // below renders for somebody with no organization to render it in.
  if (ctx && accountOnly(ctx)) {
    // Fetched here alone: the console's own pages list sessions on the
    // privacy page, which this screen stands in for, with the settings page.
    const sessions = await fetchActiveSessions();
    return (
      <LocalTimeReader reader={reader}>
        <StoreProvider
          user={seed.user}
          organizations={ctx.organizations}
          incomingInvites={ctx.incomingInvites}
          givenSeats={ctx.givenSeats}
          projectOffers={ctx.projectOffers}
          activeOrganizationId={null}
          flags={ctx.flags}
          roles={{}}
          projects={[]}
          projectsElsewhere={[]}
        >
          {/* The screen promises that an invitation shows up here: the listener
              refreshes it from the person's channel, so it does without a
              reload. */}
          <RealtimeListener liveHoldMs={liveHoldMs} readingKey={reading} />
          <AccountOnly
            email={session.email}
            authMethod={session.authMethod}
            analyticsOptIn={ctx.person.analyticsOptIn}
            signupsOpen={flagOn(ctx.flags, Flag.Signup)}
            invites={ctx.incomingInvites}
            ownedOrganizations={ownedOrganizationCount(ctx.organizations)}
            sessions={sessions}
            currentSessionId={session.sessionRowId}
            expiresAt={session.expiresAt}
          />
        </StoreProvider>
      </LocalTimeReader>
    );
  }

  const initialCollapsed = jar.get(NAV_COLLAPSED_COOKIE)?.value === "1";

  return (
    <div className="group/console theme-console flex h-svh flex-col">
      <SessionHeartbeat expiresAt={session.expiresAt} />
      <ConsoleTrailRecorder />
      <LocalTimeReader reader={reader}>
        <StoreProvider {...seed}>
          <OrganizationSync rendered={(ctx && activeOrganization(ctx)?.slug) ?? null} />
          <RealtimeListener liveHoldMs={liveHoldMs} readingKey={reading} />
          <InviteToastNotifier />
          <ConsoleUiProvider>
            <PagePrimaryActionProvider>
              <SidebarProvider initialCollapsed={initialCollapsed}>
                <SkipLink />
                <KeyboardShortcuts />
                <ConsoleSearch />
                <ProjectBanner notification={projectAnnouncement} />
                {/* The two slots a console built on this one fills may read
                    what they need themselves, after this layout's reads; each
                    streams in on its own rather than holding the shell back. */}
                <Suspense fallback={null}>
                  <ExtensionBanner />
                </Suspense>
                {/* The badge beside the organization's name, and what New
                    organization says, are slots, filled by a console built on
                    this one; the console's host goes ahead of a new
                    organization's URL, as its Settings show it. */}
                <ConsoleHeader
                  organizationBadge={
                    <Suspense fallback={null}>
                      <ExtensionOrganizationBadge />
                    </Suspense>
                  }
                />
                <div className="flex min-h-0 flex-1">
                  <ConsoleSidebar version={VERSION} />
                  <ConsoleShell
                    panels={
                      <CreatePanels
                        host={new URL(env.AUTH_URL).host}
                        notice={
                          <Suspense fallback={null}>
                            <ExtensionCreateOrganizationNotice />
                          </Suspense>
                        }
                      />
                    }
                  >
                    {children}
                  </ConsoleShell>
                </div>
              </SidebarProvider>
            </PagePrimaryActionProvider>
          </ConsoleUiProvider>
        </StoreProvider>
      </LocalTimeReader>
    </div>
  );
}
