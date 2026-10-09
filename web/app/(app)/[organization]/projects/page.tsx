import { notFound } from "next/navigation";

import { AccessDenied } from "@/components/access-denied";
import { CreateProjectAction } from "@/components/create-project";
import { ProjectTiles } from "@/components/project-tiles";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { ownerContact } from "@/lib/identity";
import { administersOrganization, organizationRoleOf } from "@/lib/organization-role";
import {
  activeOrganization,
  fetchProjectListing,
  getServerContext,
  identityContext,
} from "@/lib/server/data";
import { organizationPath } from "@/lib/slug";

import { LeaveProject } from "./_leave-project";

export const metadata = {
  title: "Projects",
};

export default async function OrganizationProjectsPage() {
  const [gate, listing, ident] = await Promise.all([
    getServerContext(),
    fetchProjectListing(),
    identityContext(),
  ]);

  if (!gate) return <ServiceUnavailable />;
  // The path names the organization: not found unless auth answered with it.
  if (gate.organizationNotFound) notFound();
  const organization = activeOrganization(gate);

  const administers = administersOrganization(organizationRoleOf(gate, ident));
  if (!organization || !ident || !administers) {
    return (
      <AccessDenied
        what="this organization's projects"
        owner={ownerContact(organization)}
        backHref={organization ? organizationPath(organization.slug) : "/console"}
      />
    );
  }

  const membershipByProject = new Map(
    (gate.memberships ?? []).map((m) => [m.projectId, m]),
  );
  // An unread listing is not an empty one: counting it as none, or saying
  // "no projects yet", would invite somebody to make a second of one they have.
  const projects = listing.kind === "ok" ? listing.projects : null;

  return (
    <>
      <PageHeader
        title="Projects"
        count={projects?.length}
        action={<CreateProjectAction />}
      />

      <div className="grid gap-6">
        {/* The tiles, with Leave beside a project the person holds a seat
            in: the one place a seat is given up. */}
        <section className="grid gap-3" aria-label="Projects">
          {projects === null ? (
            <Card className="overflow-hidden p-0 gap-0">
              <div className="grid gap-1 p-4">
                <p className="text-sm font-medium">The projects could not be loaded.</p>
                <p className="text-sm text-muted-foreground">Try again in a moment.</p>
              </div>
            </Card>
          ) : projects.length === 0 ? (
            <Card className="overflow-hidden p-0 gap-0">
              <div className="grid gap-1 p-4">
                <p className="text-sm font-medium">No projects yet.</p>
                <p className="text-sm text-muted-foreground">
                  Create the first one with New project, above.
                </p>
              </div>
            </Card>
          ) : (
            <ProjectTiles
              organization={organization.slug}
              projects={projects}
              action={(p) =>
                membershipByProject.has(p.id) ? (
                  <LeaveProject projectId={p.id} name={p.name} />
                ) : null
              }
            />
          )}
          <p className="text-xs text-muted-foreground">
            Each project keeps its own API keys and members.
          </p>
        </section>
      </div>
    </>
  );
}
