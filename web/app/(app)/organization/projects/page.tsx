import Link from "next/link";
import { ArrowUpRight } from "lucide-react";

import { AccessDenied } from "@/components/access-denied";
import { CopyButton } from "@/components/copy-button";
import { CreateProjectAction } from "@/components/create-project";
import { DataTable, THead, Th, Td } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { RoleBadge } from "@/components/role-badge";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ownerContact } from "@/lib/identity";
import { administersOrganization, organizationRoleOf } from "@/lib/organization-role";
import {
  activeOrganization,
  fetchProjects,
  getServerContext,
  identityContext,
} from "@/lib/server/data";

import { LeaveProject } from "./_leave-project";

export const metadata = {
  title: "Projects",
};

export default async function OrganizationProjectsPage() {
  const [gate, projects, ident] = await Promise.all([
    getServerContext(),
    fetchProjects(),
    identityContext(),
  ]);

  if (!gate) return <ServiceUnavailable />;
  const organization = activeOrganization(gate);

  const administers = administersOrganization(organizationRoleOf(gate, ident));
  if (!organization || !ident || !administers) {
    return (
      <AccessDenied
        what="this organization's projects"
        owner={ownerContact(organization)}
      />
    );
  }

  const membershipByProject = new Map(
    (gate.memberships ?? []).map((m) => [m.projectId, m]),
  );

  return (
    <>
      <PageHeader
        title="Projects"
        action={<CreateProjectAction />}
      />

      <div className="grid gap-6">
        <section className="grid gap-3">
          <div className="flex items-center gap-2">
            <h2 className="text-sm font-medium">Owned by this organization</h2>
            <Badge variant="secondary" className="text-xs">
              {projects.length}
            </Badge>
          </div>
          {projects.length === 0 ? (
            <Card className="overflow-hidden p-0 gap-0">
              <div className="grid gap-1 p-4">
                <p className="text-sm font-medium">No projects yet.</p>
                <p className="text-sm text-muted-foreground">
                  Create the first one with New project, above.
                </p>
              </div>
            </Card>
          ) : (
            <DataTable>
              <THead>
                <Th>Project</Th>
                <Th>Your role</Th>
                <Th className="text-right" />
              </THead>
              <tbody>
                {projects.map((p) => {
                  const membership = membershipByProject.get(p.id);
                  return (
                    <tr key={p.id}>
                      <Td className="font-medium">
                        <div>
                          <span>{p.name}</span>
                          <div className="flex items-center gap-1.5">
                            <span className="font-mono text-xs font-normal text-muted-foreground">
                              {p.id}
                            </span>
                            <CopyButton value={p.id} label="Project ID" />
                          </div>
                        </div>
                      </Td>
                      <Td className="text-xs text-muted-foreground">
                        <RoleBadge role={p.role} />
                      </Td>
                      <Td className="text-right">
                        <div className="flex items-center justify-end gap-3">
                          <Button asChild size="sm" variant="outline" className="h-8 text-xs gap-1">
                            <Link href={`/${p.id}`}>
                              Open
                              <ArrowUpRight className="size-3.5" />
                            </Link>
                          </Button>
                          {membership && (
                            <LeaveProject projectId={p.id} name={p.name} />
                          )}
                        </div>
                      </Td>
                    </tr>
                  );
                })}
              </tbody>
            </DataTable>
          )}
          <p className="text-xs text-muted-foreground">
            Each project keeps its own API keys and members.
          </p>
        </section>
      </div>
    </>
  );
}
