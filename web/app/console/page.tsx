import { redirect } from "next/navigation";

import { PaperShell } from "@/components/paper-shell";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { administersOrganization } from "@/lib/organization-role";
import { activeOrganization, fetchProjectListing, getServerContext } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import { organizationPath, projectPath } from "@/lib/slug";

import { NameOrganizationForm } from "./_name-organization";

export default async function ConsoleEntry({
  searchParams,
}: {
  searchParams: Promise<{ organization?: string | string[] }>;
}) {
  const session = await getServerSession();

  if (!session) redirect("/auth/logout");

  // Somebody in no organization has no project to land on. Any (app) route
  // gives them their account; this one names what is there for them.
  const ctx = await getServerContext();
  if (ctx && ctx.activeOrganizationId === null) redirect("/account/notifications");

  // ⚠ An organization is named before the console opens to its owner, as a
  // Vercel team is named when it is made. Provisioned at first sign-in, it has
  // a placeholder slug nobody chose, which would otherwise be the first
  // address they saw; its first name gives it a URL, and both are theirs to
  // change on Settings. Asked here, under `/console`, where the address bar
  // spells no slug at all. Nobody else is in it: auth invites nobody to an
  // unnamed organization.
  //
  // `?organization=` is `[organization]/layout.tsx` saying which one it sent
  // the owner from. This path names no organization, so `/me` answers the
  // cookie's — another organization, when the owner opened an unnamed one of
  // theirs from inside it — and the question would otherwise be asked of the
  // wrong one, or not at all.
  const { organization: sentFrom } = await searchParams;
  const sent =
    typeof sentFrom === "string"
      ? ctx?.organizations.find((o) => o.organizationId === sentFrom)
      : undefined;
  const standing = sent ?? (ctx ? activeOrganization(ctx) : null);
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

  const listing = await fetchProjectListing();
  // This route has no layout, and it is the only caller of
  // `ServiceUnavailable` outside the console shell — the shell is what gives
  // the other fifteen their gutter, so the card carries no horizontal padding
  // of its own and lands flush against the viewport here. With the card's own
  // `py-8` this totals the `px-8 py-24` of the bare `error` and `not-found`.
  if (listing.kind === "unavailable") {
    return (
      <div className="px-8 py-16">
        <ServiceUnavailable />
      </div>
    );
  }

  // Nothing to open: the projects page, where a project is made, for whoever
  // may make one. It refuses a member, so a member left with no seat in this
  // organization — someone whose project was handed away — was sent straight
  // to ACCESS DENIED; they get the overview, open to everyone in it.
  const [first] = listing.projects;
  // A listing answered, so `/me` did, and named the organization it is for.
  const organization = ctx ? activeOrganization(ctx) : null;
  redirect(
    !organization
      ? "/account/notifications"
      : first
        ? projectPath(organization.slug, first.slug)
        : organizationPath(
            organization.slug,
            administersOrganization(organization.role) ? "/projects" : "",
          ),
  );
}
