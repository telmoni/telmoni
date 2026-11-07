import { notFound } from "next/navigation";

import { DataTable, THead, Td, Th } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
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

/** The plan's numbers, in one sentence, where it holds any. */
function planSentence(plan: Limits["plan"]): string {
  const parts = [
    plan.spans !== null && `${formatCount(plan.spans)} spans a month`,
    plan.content_bytes !== null && `${shown(plan.content_bytes, MB)} of content`,
    plan.alerts !== null && `${formatCount(plan.alerts)} alerts and objectives`,
    plan.alert_deliveries_per_day !== null &&
      `${formatCount(plan.alert_deliveries_per_day)} alert deliveries a day`,
  ].filter(Boolean);
  return parts.length === 0
    ? "No plan limits this organization: a cap is the only number a project is held to."
    : `The plan holds ${parts.join(", ")}, shared by every project.`;
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

  return (
    <>
      <PageHeader title="Limits" />
      <div className="grid gap-page">
        <Section
          card
          title="Each project's share"
          description={`${limits ? planSentence(limits.plan) : ""} An owner or admin may cap a project at less than the plan, so one project cannot use what the others need; a project with no cap shares the pool. Past its cap, that project alone is held as the organization is past its plan: its spans sampled, its content kept as metadata, no new alert, monitor or objective, and its alerts' deliveries held to the feed until the day turns.`}
        >
          {limits === null ? (
            <p className="text-muted-foreground">The limits could not be read. Reload to try again.</p>
          ) : limits.projects.length === 0 ? (
            <p className="text-muted-foreground">There are no projects to divide the pool among yet.</p>
          ) : (
            <DataTable>
              <THead>
                <Th>Project</Th>
                <Th>Spans this month</Th>
                <Th>Content</Th>
                <Th>Alerts</Th>
                <Th>Monitors</Th>
                <Th>Objectives</Th>
                <Th>Deliveries today</Th>
                {sets && <Th />}
              </THead>
              <tbody>
                {limits.projects.map((project: Project) => (
                  <tr key={project.id}>
                    <Td className="font-medium">{project.name}</Td>
                    <Td className="whitespace-nowrap text-xs">
                      {share(project.used.spans, project.caps.spans)}
                    </Td>
                    <Td className="whitespace-nowrap text-xs">
                      {share(project.used.content_bytes, project.caps.content_bytes, MB)}
                    </Td>
                    <Td className="whitespace-nowrap text-xs">
                      {share(project.used.alerts, project.caps.alerts)}
                    </Td>
                    <Td className="whitespace-nowrap text-xs">
                      {share(project.used.monitors, project.caps.monitors)}
                    </Td>
                    <Td className="whitespace-nowrap text-xs">
                      {share(project.used.objectives, project.caps.objectives)}
                    </Td>
                    <Td className="whitespace-nowrap text-xs">
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
      </div>
    </>
  );
}
