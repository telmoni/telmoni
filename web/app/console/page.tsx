import { redirect } from "next/navigation";

import { PaperShell } from "@/components/paper-shell";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { activeOrganization, getServerContext } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import { organizationPath } from "@/lib/slug";

import { NameOrganizationForm } from "./_name-organization";

export default async function ConsoleEntry({
  searchParams,
}: {
  searchParams: Promise<{ organization?: string | string[] }>;
}) {
  const session = await getServerSession();

  if (!session) redirect("/auth/logout");

  const ctx = await getServerContext();
  // This route has no layout, and it is the only caller of
  // `ServiceUnavailable` outside the console shell — the shell is what gives
  // the other fifteen their gutter, so the card carries no horizontal padding
  // of its own and lands flush against the viewport here. With the card's own
  // `py-8` this totals the `px-8 py-24` of the bare `error` and `not-found`.
  if (!ctx) {
    return (
      <div className="px-8 py-16">
        <ServiceUnavailable />
      </div>
    );
  }

  // Somebody in no organization has no overview to land on. Any (app) route
  // gives them their account; this one names what is there for them.
  if (ctx.activeOrganizationId === null) redirect("/account/notifications");

  // ⚠ An organization is named before the console opens to its owner, as a
  // Vercel team is named when it is made. Provisioned at first sign-in, it has
  // a placeholder slug nobody chose, which would otherwise be the first
  // address they saw; its first name gives it a URL, and both are theirs to
  // change on Settings. Asked here, under `/console`, where the address bar
  // spells no slug at all. Nobody else is in it: auth invites nobody to an
  // unnamed organization.
  //
  // `?organization=` is `[organization]/layout.tsx` saying which one it sent
  // the owner from. This path names no organization, so `/me` would answer
  // the cookie's — another organization, when the owner opened an unnamed one
  // of theirs from inside it — and the question would otherwise be asked of
  // the wrong one, or not at all.
  const { organization: sentFrom } = await searchParams;
  const sent =
    typeof sentFrom === "string"
      ? ctx.organizations.find((o) => o.organizationId === sentFrom)
      : undefined;
  const organization = activeOrganization(ctx);
  const standing = sent ?? organization;
  if (standing && !standing.name?.trim() && standing.role === "owner") {
    return (
      <PaperShell signedIn>
        <main className="mx-auto grid w-full max-w-xl gap-6 px-3.5 py-10">
          <Card data-testid="name-organization">
            <h1 className="text-lg font-semibold">Name your organization</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              This is what everyone in it sees: your company or group, not a person&apos;s
              name. Its address on Telmoni follows from it, and you can change both in
              Settings.
            </p>
            <NameOrganizationForm organizationId={standing.organizationId} />
          </Card>
        </main>
      </PaperShell>
    );
  }

  // ⚠ The organization's overview, never a project of it: as Vercel opens on
  // a team's overview, which lists the projects and is open to everybody in
  // the organization, where the projects page refuses a member. Which
  // organization is auth's answer: the one the cookie remembers, else the
  // person's default — and a sign-in clears the cookie (`auth/callback`), so
  // signing in opens the default, while within a session this returns to
  // where the console stood: after a restore, an accepted invitation, a slug
  // that moved under an open tab.
  redirect(organization ? organizationPath(organization.slug) : "/account/notifications");
}
