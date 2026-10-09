import { redirect } from "next/navigation";

import { ServiceUnavailable } from "@/components/service-unavailable";
import { activeOrganization, getServerContext } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import { organizationPath } from "@/lib/slug";

export default async function ConsoleEntry() {
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

  // ⚠ The organization's overview, never a project of it: as Vercel opens on
  // a team's overview, the one page every member of the organization may
  // open, where the projects page refuses a member; their projects are a
  // press away in the header's switcher. Nothing is asked first: an
  // organization is provisioned already named after its owner, as Vercel and
  // Cloudflare name a new account, and renamed on Settings. Which
  // organization is auth's answer: the one the cookie remembers, else the
  // person's default — and a sign-in clears the cookie (`auth/callback`), so
  // signing in opens the default, while within a session this returns to
  // where the console stood: after an accepted invitation, or a slug that
  // moved under an open tab. Somebody in no organization has no overview to
  // land on; their account names what is there for them.
  const organization = activeOrganization(ctx);
  redirect(organization ? organizationPath(organization.slug) : "/account/notifications");
}
