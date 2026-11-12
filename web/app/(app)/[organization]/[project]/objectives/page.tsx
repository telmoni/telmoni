import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { DataTable, THead, Td, Th } from "@/components/data-table";
import { FirstRun, FirstRunStep } from "@/components/first-run";
import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { SeverityMark } from "@/components/severity-mark";
import { SEVERITY_LABEL, amount, connectionChoices, promiseWords } from "@/lib/alerts";
import { ownerContact } from "@/lib/identity";
import { OBJECTIVES_HEADING, lastsWords } from "@/lib/objectives";
import {
  activeOrganization,
  fetchAlerts,
  fetchConnectors,
  fetchHistory,
  fetchObjectives,
} from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";
import { RuleAggregation, RuleMeasure } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

import { AlertActions } from "../alerts/_alert-actions";
import { NewObjective } from "./_objective-panel";

export const metadata = { title: "Objectives" };

const PERCENT = new Intl.NumberFormat("en-US", { style: "percent", maximumFractionDigits: 1 });

/** A burn as the page reads it: `14.4×`. */
const burnWords = (burn: number) => amount(RuleMeasure.Burn, RuleAggregation.Avg, burn);

// The project's objectives: each one agent's traces promised to succeed, or to
// end inside a time, over its days, with the budget its misses have left and
// how fast its last hour and six hours spend it. Each is the burn alert it
// is: its incidents, pauses and windows stand under it on the Alerts page.
export default async function ObjectivesPage({ params }: PageProps<"/[organization]/[project]/objectives">) {
  const { organization, project: slug } = await params;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;
  const [access, standings, listing, history] = await Promise.all([
    fetchAlerts(project.id),
    fetchObjectives(project.id),
    fetchConnectors(project.id),
    fetchHistory(project.id),
  ]);
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's objectives"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }
  const connections = listing ? connectionChoices(listing.connections) : null;
  const objectives = access.alerts.flatMap((alert) => (alert.objective ? [{ alert, objective: alert.objective }] : []));
  const standingOf = new Map((standings ?? []).map((standing) => [standing.rule_id, standing]));
  const alertsHref = `${projectPath(organization, slug)}/alerts`;

  return (
    <>
      <PageHeader
        title="Objectives"
        headingId={OBJECTIVES_HEADING}
        info="Each promises a share of one agent's traces succeed, or end inside a time, over its days. The traces that miss it spend its error budget; it pages when its last hour spends the budget 14.4× as fast as it lasts, and warns at 6× over its last six hours. Each counts as one of the plan's alerts, and its incidents stand under it on the Alerts page."
        action={objectives.length > 0 ? <NewObjective projectId={project.id} connections={connections} history={history} /> : undefined}
      />
      <div className="grid gap-page">
        <section className="grid gap-gutter">
        {objectives.length === 0 ? (
          <FirstRun
            title="Set a reliability target for your agent"
            lead="Choose what good looks like. See how much room your agent has left for failures or slow traces."
          >
            <FirstRunStep
              n={1}
              title="Choose an agent and a target"
              note="Promise a share of its traces will succeed, or finish within a time limit, over a window of days."
            >
              <div className="flex flex-wrap gap-gutter">
                <NewObjective projectId={project.id} connections={connections} history={history} />
              </div>
            </FirstRunStep>
            <FirstRunStep
              n={2}
              title="Watch its error budget"
              note="Once created, your objective appears here. Traces that miss the target spend its budget; fast burn opens an incident."
            />
            <FirstRunStep
              n={3}
              title="Choose where incidents reach you"
              note="Incidents appear in your feed. Owners and admins connect Slack or a webhook; select a connector on your objective to notify your team too."
            >
              <Link href={`${projectPath(organization, slug)}/connectors`} className="w-fit text-sm text-muted-foreground underline underline-offset-4 hover:text-foreground">
                View connectors
              </Link>
            </FirstRunStep>
          </FirstRun>
        ) : (
          <>
            {standings === null && (
              <NoticeCard title="Where they stand could not be read." body="Reload to try again." />
            )}
            <DataTable>
              <THead>
                <Th>Name</Th>
                <Th>Promise</Th>
                <Th className="text-right">Budget left</Th>
                <Th className="text-right">Last hour</Th>
                <Th className="text-right">Last six hours</Th>
                <Th />
              </THead>
              <tbody>
                {objectives.map(({ alert, objective }) => {
                  const standing = standingOf.get(alert.id);
                  // Past 1 the budget is gone before its days are.
                  const burn = standing ? Math.max(standing.hour_burn, standing.six_hour_burn) : 0;
                  const lasts = burn > 1 ? lastsWords(objective.days, burn) : null;
                  return (
                    <tr key={alert.id}>
                      <Td className="font-medium whitespace-nowrap">
                        <span className="flex items-center gap-gutter">
                          {alert.name}
                          <SeverityMark severity={alert.severity} label={SEVERITY_LABEL[alert.severity]} />
                        </span>
                        <Link
                          href={`${alertsHref}#alert-${alert.id}`}
                          className="block text-xs font-normal text-muted-foreground underline-offset-4 hover:underline"
                        >
                          Its incidents
                        </Link>
                      </Td>
                      <Td className="text-xs text-muted-foreground">
                        {promiseWords(objective, alert.filters.agent?.[0] ?? "")}
                      </Td>
                      <Td className="text-right whitespace-nowrap tabular-nums">
                        {standing ? (
                          <>
                            <span className={cn(standing.budget_left < 0 && "text-destructive")}>
                              {PERCENT.format(standing.budget_left)}
                            </span>
                            <span className="block text-xs text-muted-foreground">
                              {standing.missed.toLocaleString("en-US")} of {standing.traces.toLocaleString("en-US")} traces
                              missed, {standing.days} days
                            </span>
                          </>
                        ) : (
                          "—"
                        )}
                      </Td>
                      <Td className="text-right whitespace-nowrap tabular-nums">
                        {standing ? burnWords(standing.hour_burn) : "—"}
                      </Td>
                      <Td className="text-right whitespace-nowrap tabular-nums">
                        {standing ? burnWords(standing.six_hour_burn) : "—"}
                        {lasts && (
                          <span className="block text-xs text-muted-foreground">Gone in {lasts} at this rate</span>
                        )}
                      </Td>
                      <Td className="text-right whitespace-nowrap">
                        <AlertActions
                          projectId={project.id}
                          alert={alert}
                          connections={connections}
                          history={history}
                          landing={OBJECTIVES_HEADING}
                        />
                      </Td>
                    </tr>
                  );
                })}
              </tbody>
            </DataTable>
          </>
        )}
        </section>
      </div>
    </>
  );
}
