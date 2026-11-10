import { notFound } from "next/navigation";

import { DataTable, THead, Td, Th } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { MB } from "@/lib/limits";
import { administers } from "@/lib/roles";
import { activeOrganization, getServerContext, identityContext } from "@/lib/server/data";
import { type Limits, fetchLimits } from "@/lib/server/entities/limits";
import { formatCount } from "@/lib/traces";

import { EditCaps } from "./_caps";

export const metadata = { title: "Limits" };

type Project = Limits["projects"][number];

/** A count, or content in MB to one place. */
function shown(n: number, unit?: number): string {
  return unit ? `${(n / unit).toLocaleString("en-US", { maximumFractionDigits: 1 })} MB` : formatCount(n);
}

/** What a project has used of a limit, beside its cap where it has one. */
function share(used: number | null, cap: number | null, unit?: number): string {
  const spent = used === null ? "—" : shown(used, unit);
  return cap === null ? spent : `${spent} of ${shown(cap, unit)}`;
}

export default async function LimitsPage() {
  const [gate, ident, limits] = await Promise.all([
    getServerContext(),
    identityContext(),
    fetchLimits(),
  ]);
  if (!gate) return <ServiceUnavailable />;
  if (gate.organizationNotFound) notFound();
  const organization = activeOrganization(gate);
  if (!organization || !ident) notFound();
  const sets = administers(ident.role);
  const pool = limits
    ? [
        { label: "Spans a month", value: limits.plan.spans },
        { label: "Content held", value: limits.plan.content_bytes, unit: MB },
        { label: "Alerts and objectives", value: limits.plan.alerts },
        { label: "Alert deliveries a day", value: limits.plan.alert_deliveries_per_day },
      ].flatMap(({ label, value, unit }) =>
        value === null ? [] : [{ label, value: shown(value, unit) }],
      )
    : [];

  return (
    <>
      <PageHeader title="Limits" />
      <div className="grid gap-page">
        {limits && (
          <Section
            title="Organization pool"
            description="Shared by every project. A project cap limits its use of the pool; it does not reserve capacity."
          >
            {pool.length > 0 ? (
              <dl className="grid auto-rows-fr gap-page sm:grid-cols-2 xl:grid-cols-4">
                {pool.map(({ label, value }) => (
                  <Card key={label} className="gap-gutter">
                    <dt className="text-sm text-muted-foreground">{label}</dt>
                    <dd className="text-xl font-medium tabular-nums">{value}</dd>
                  </Card>
                ))}
              </dl>
            ) : (
              <p className="text-sm text-muted-foreground">
                This organization has no plan limits. You can still set individual project caps.
              </p>
            )}
          </Section>
        )}

        <Section
          title="Projects"
          description={`Values show usage, or usage of a cap when one is set. Projects without a cap share the pool.${sets ? " Choose Set caps to change a project's limits." : " An organization owner or admin can change project caps."}`}
        >
          {limits === null ? (
            <p className="text-muted-foreground">The limits could not be read. Reload to try again.</p>
          ) : limits.projects.length === 0 ? (
            <p className="text-muted-foreground">There are no projects to divide the pool among yet.</p>
          ) : (
            <DataTable aria-label="Project usage and caps" className="tabular-nums">
              <THead>
                <Th>Project</Th>
                <Th>Spans this month</Th>
                <Th>Content held</Th>
                <Th>Alerts</Th>
                <Th>Monitors</Th>
                <Th>Objectives</Th>
                <Th>Alert deliveries today</Th>
                {sets && <Th><span className="sr-only">Actions</span></Th>}
              </THead>
              <tbody>
                {limits.projects.map((project: Project) => (
                  <tr key={project.id}>
                    <Td className="font-medium">{project.name}</Td>
                    <Td className="whitespace-nowrap">
                      {share(project.used.spans, project.caps.spans)}
                    </Td>
                    <Td className="whitespace-nowrap">
                      {share(project.used.content_bytes, project.caps.content_bytes, MB)}
                    </Td>
                    <Td className="whitespace-nowrap">
                      {share(project.used.alerts, project.caps.alerts)}
                    </Td>
                    <Td className="whitespace-nowrap">
                      {share(project.used.monitors, project.caps.monitors)}
                    </Td>
                    <Td className="whitespace-nowrap">
                      {share(project.used.objectives, project.caps.objectives)}
                    </Td>
                    <Td className="whitespace-nowrap">
                      {project.caps.alert_deliveries_per_day === null
                        ? "Shares the pool"
                        : share(
                            project.used.alert_deliveries_today,
                            project.caps.alert_deliveries_per_day,
                          )}
                    </Td>
                    {sets && (
                      <Td className="text-right">
                        <EditCaps
                          organizationId={organization.organizationId}
                          projectId={project.id}
                          name={project.name}
                          caps={project.caps}
                        />
                      </Td>
                    )}
                  </tr>
                ))}
              </tbody>
            </DataTable>
          )}
        </Section>

        <details className="text-sm">
          <summary className="w-fit cursor-pointer font-medium">
            What happens at a project cap?
          </summary>
          <p className="mt-gutter text-muted-foreground">
            Only that project is held back. Other projects can keep using the available pool.
          </p>
          <dl className="mt-gutter grid gap-page sm:grid-cols-2 xl:grid-cols-4">
            <div className="grid content-start gap-1">
              <dt className="font-medium">Spans</dt>
              <dd className="text-muted-foreground">One trace in ten is kept, with no grace period.</dd>
            </div>
            <div className="grid content-start gap-1">
              <dt className="font-medium">Content</dt>
              <dd className="text-muted-foreground">New content is dropped; span metadata is kept.</dd>
            </div>
            <div className="grid content-start gap-1">
              <dt className="font-medium">Alerts, monitors and objectives</dt>
              <dd className="text-muted-foreground">New ones are refused until there is room or the cap is raised.</dd>
            </div>
            <div className="grid content-start gap-1">
              <dt className="font-medium">Alert deliveries</dt>
              <dd className="text-muted-foreground">Incidents reach the feed alone until midnight UTC.</dd>
            </div>
          </dl>
        </details>
      </div>
    </>
  );
}
