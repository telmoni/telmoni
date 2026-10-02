import { redirect } from "next/navigation";

import { ServiceUnavailable } from "@/components/service-unavailable";
import { administersOrganization } from "@/lib/organization-role";
import { activeOrganization, fetchProjectListing, getServerContext } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import { organizationPath, projectPath } from "@/lib/slug";

export default async function ConsoleEntry() {
  const session = await getServerSession();

  if (!session) redirect("/auth/logout");

  // Somebody in no organization has no project to land on. Any (app) route
  // gives them their account; this one names what is there for them.
  const ctx = await getServerContext();
  if (ctx && ctx.activeOrganizationId === null) redirect("/account/notifications");

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
