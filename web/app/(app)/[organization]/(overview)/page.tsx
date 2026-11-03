import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { BookOpen, Send, UserPlus } from "lucide-react";
import Link from "next/link";

import { CreateProjectAction } from "@/components/create-project";
import { NoticeCard } from "@/components/notice-card";
import { ProjectTiles } from "@/components/project-tiles";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Button } from "@/components/ui/button";
import { organizationLabel } from "@/lib/identity";
import { administers, owns } from "@/lib/roles";
import { activeOrganization, fetchProjectListing, getServerContext } from "@/lib/server/data";
import { DOCS_URL } from "@/lib/site";
import { organizationPath } from "@/lib/slug";

import { Overview } from "../../_overview";
import { LeaveOrganization } from "../_leave-organization";

// In a route group of its own so that whatever stands in for a page while it
// loads can wrap the overview alone: beside the organization's layout it
// would wrap the project's layout too, and stream that layout's redirect and
// not-found after a page already answered 200.

// The tab is titled as the page is, by the organization's name. The context
// is read again for it, a second call auth answers from its cache.
export async function generateMetadata(): Promise<Metadata> {
  const gate = await getServerContext();
  const organization = gate && !gate.organizationNotFound ? activeOrganization(gate) : null;
  return { title: organization ? organizationLabel(organization) : "Overview" };
}

// A welcome to the organization: where you are and what to do first, its
// projects by name, and the way out.
export default async function OrganizationOverviewPage() {
  const [gate, listing] = await Promise.all([getServerContext(), fetchProjectListing()]);
  // The path names the organization: not found unless auth answered with it.
  if (gate?.organizationNotFound) notFound();
  const organization = gate ? activeOrganization(gate) : null;
  if (!organization) return <ServiceUnavailable />;

  // The organization's own name — never the name or the address of the
  // person who holds it. See `organizationLabel`.
  const name = organizationLabel(organization);

  const isOwner = owns(organization.role);
  const isAdmin = administers(organization.role);
  // Anybody but the owner may leave. The owner hands the organization to an
  // admin first — the organization cannot be left with nobody holding it.
  const canLeave = !isOwner;
  // An unread listing is not an empty one: "no projects yet" would invite a second of one.
  const projects = listing.kind === "ok" ? listing.projects : null;
  const quick = [
    ...(isAdmin ? [{ href: organizationPath(organization.slug, "/members"), label: "Invite a member", icon: UserPlus }] : []),
    { href: `${DOCS_URL}/api/sdks`, label: "Send a first trace", icon: Send },
    { href: DOCS_URL, label: "Read the docs", icon: BookOpen },
  ];

  return (
    <Overview
      title="Welcome"
      sections={
        <>
          <section aria-label="Getting around" className="grid gap-gutter">
            <p className="text-base">
              You&rsquo;re working in <span className="font-medium">{name}</span>.
            </p>
            <div className="flex flex-wrap gap-gutter">
              {/* As the Projects page draws it, its shortcut with it. */}
              <CreateProjectAction />
              {quick.map(({ href, label, icon: Icon }) => (
                <Button key={label} asChild variant="outline" size="sm">
                  <Link href={href}>
                    <Icon className="size-4" aria-hidden="true" />
                    {label}
                  </Link>
                </Button>
              ))}
            </div>
          </section>

          <section aria-labelledby="overview-projects" className="grid gap-gutter">
            <h2 id="overview-projects" className="text-base font-medium">
              Projects
            </h2>
            {projects === null ? (
              <NoticeCard title="The projects could not be loaded." body="Try again in a moment." />
            ) : projects.length === 0 ? (
              <NoticeCard
                title="No projects yet."
                body={
                  isAdmin
                    ? "Create the first one with New project, above."
                    : "An owner or admin creates them."
                }
              />
            ) : (
              <ProjectTiles organization={organization.slug} projects={projects} roles={false} />
            )}
          </section>

          {canLeave && (
            <LeaveOrganization organizationId={organization.organizationId} name={name} />
          )}
        </>
      }
    />
  );
}
