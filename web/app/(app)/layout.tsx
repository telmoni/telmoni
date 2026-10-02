import { ConsoleUiProvider } from "@/components/console-ui-context";
import { cookies } from "next/headers";
import { redirect } from "next/navigation";

import { ConsoleHeader } from "@/components/console-header";
import { ConsoleShell } from "@/components/console-shell";
import { ConsoleSidebar } from "@/components/console-sidebar";
import { KeyboardShortcuts } from "@/components/keyboard-shortcuts";
import { SkipLink } from "@/components/skip-link";
import { NAV_COLLAPSED_COOKIE } from "@/lib/console-nav";
import { PagePrimaryActionProvider } from "@/components/page-action";
import { SidebarProvider } from "@/components/sidebar-context";
import { ProjectBanner } from "@/components/project-banner";
import { ExtensionBanner } from "@/components/extension/banner";
import { InviteToastNotifier } from "@/components/invite-toast-notifier";
import { OrganizationSync } from "@/components/organization-sync";
import { RealtimeListener } from "@/components/realtime-listener";
import { PageHeaderProvider } from "@/components/page-header-context";
import {
  activeOrganization,
  fetchActiveSessions,
  getServerContext,
  fetchProjectAnnouncement,
} from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import { storeSeed } from "@/lib/server/store-seed";
import { Flag, flagOn } from "@/lib/flags";
import { StoreProvider } from "@/lib/store/provider";
import { accountOnlyReason } from "@/lib/account-only";
import { ownedOrganizationLabels } from "@/lib/identity";
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

  // Every route under (app) answers this way while a gate is up, so no page
  // below renders for somebody with no organization to render it in.
  const reason = ctx ? accountOnlyReason(ctx) : null;
  if (ctx && reason) {
    // Fetched here alone: the console's own pages list sessions on the
    // privacy page, which this screen stands in for, with the settings page.
    const sessions = await fetchActiveSessions();
    return (
      <StoreProvider
        user={seed.user}
        organizations={ctx.organizations}
        incomingInvites={ctx.incomingInvites}
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
        <RealtimeListener />
        <AccountOnly
          reason={reason}
          email={session.email}
          authMethod={session.authMethod}
          analyticsOptIn={ctx.person.analyticsOptIn}
          signupsOpen={flagOn(ctx.flags, Flag.Signup)}
          invites={ctx.incomingInvites}
          ownedOrganizations={ownedOrganizationLabels(ctx.organizations)}
          deletedOrganizations={ctx.deletedOrganizations}
          sessions={sessions}
          currentSessionId={session.sessionRowId}
          expiresAt={session.expiresAt}
          needsReseal={session.needsReseal}
        />
      </StoreProvider>
    );
  }

  const jar = await cookies();
  const initialCollapsed = jar.get(NAV_COLLAPSED_COOKIE)?.value === "1";

  return (
    <div className="group/console theme-console flex h-svh flex-col">
      <SessionHeartbeat
        expiresAt={session.expiresAt}
        needsReseal={session.needsReseal}
      />
      <ConsoleTrailRecorder />
      <StoreProvider {...seed}>
        <OrganizationSync rendered={(ctx && activeOrganization(ctx)?.slug) ?? null} />
        <RealtimeListener />
        <InviteToastNotifier />
        <ConsoleUiProvider>
          <PageHeaderProvider>
          <PagePrimaryActionProvider>
            <SidebarProvider initialCollapsed={initialCollapsed}>
              <SkipLink />
              <KeyboardShortcuts />
              <ConsoleSearch />
              <ProjectBanner notification={projectAnnouncement} />
              <ExtensionBanner />
              <ConsoleHeader />
              <div className="flex min-h-0 flex-1 md:pl-3.5">
                <ConsoleSidebar />
                <ConsoleShell>{children}</ConsoleShell>
              </div>
            </SidebarProvider>
          </PagePrimaryActionProvider>
        </PageHeaderProvider>
        </ConsoleUiProvider>
      </StoreProvider>
    </div>
  );
}
