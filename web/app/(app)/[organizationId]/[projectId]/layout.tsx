import { notFound } from "next/navigation";

import { organizationLabel } from "@/lib/identity";
import { organizationMatches, projectMatches } from "@/lib/slug";
import { fetchProjectListing } from "@/lib/server/data";
import { getServerContext } from "@/lib/server/entities/organization";
import { fetchProjectsEverywhere } from "@/lib/server/entities/projects";

import { ProjectElsewhere } from "./_elsewhere";

export default async function ProjectLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ organizationId?: string; projectId?: string }>;
}) {
  const { organizationId = "", projectId = "" } = await params;

  const listing = await fetchProjectListing();
  if (listing.kind === "ok" && !listing.projects.some((p) => projectMatches(p, projectId))) {
    // A tab left open on a project that has since been handed to another
    // organization landed here as a bare 404 for its previous owner, who can
    // still open it. Offered as a button, not followed: switching
    // organizations changes what the console reads, so the person says so.
    const [everywhere, ctx] = await Promise.all([fetchProjectsEverywhere(), getServerContext()]);
    const there =
      everywhere.kind === "ok" ? everywhere.projects.find((p) => projectMatches(p, projectId)) : undefined;
    // Only somewhere the switch can go: not the organization already active,
    // and not one reached by a project seat alone, which it refuses.
    const targetOrg = organizationId || ctx?.activeOrganizationId;
    const switchable =
      there !== undefined &&
      there.organizationId !== targetOrg &&
      (ctx?.organizations.some((o) => organizationMatches(o, there.organizationId)) ?? false);
    if (!there || !switchable) notFound();
    return (
      <ProjectElsewhere
        projectId={there.id}
        projectName={there.name}
        organizationId={there.organizationId}
        organizationLabel={organizationLabel({
          name: there.organizationName,
          ownerEmail: there.organizationOwnerEmail,
        })}
      />
    );
  }
  return children;
}
