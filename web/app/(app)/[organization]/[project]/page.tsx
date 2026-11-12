import type { Metadata } from "next";
import Link from "next/link";
import type { ReactNode } from "react";

import { TraceFirstRun } from "@/components/first-run";
import { LocalTime } from "@/components/local-time";
import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { StatusMark } from "@/components/status-badge";
import { Card } from "@/components/ui/card";
import { agentHref, openIncidentsOf, watchersOf } from "@/lib/agents";
import { fetchProjectBySlug } from "@/lib/server/data";
import { fetchAgents } from "@/lib/server/entities/agents";
import { fetchAlerts } from "@/lib/server/entities/alerts";
import { MAX_DAYS } from "@/lib/list-query";
import { administers } from "@/lib/roles";
import { DOCS_URL } from "@/lib/site";
import { projectPath } from "@/lib/slug";
import { changeWords, counted, formatCost, formatCount, traceHref } from "@/lib/traces";
import { ChangeKind } from "@/lib/types/enums";

import { VersionsHost, VersionsOpener } from "./_versions-panel";

// The tab is titled by the project's name, so a tab among several says which.
export async function generateMetadata({
  params,
}: PageProps<"/[organization]/[project]">): Promise<Metadata> {
  const { project: slug } = await params;
  const project = await fetchProjectBySlug(slug);
  return { title: project ? project.name : "Agents" };
}

/** The order an agent's changes stand in under its name: the contract's, the release first. */
const KINDS = Object.values(ChangeKind);

/** What an agent's traces carried, the most first: two named, the rest counted. */
function listWords(one: string, many: string, values: string[]): string {
  const rest = values.length - 2;
  const named = values.slice(0, 2).join(", ");
  return `${values.length === 1 ? one : many} ${named}${rest > 0 ? ` +${rest}` : ""}`;
}

/** One figure on an agent's card: what it counts over its value. */
function Stat({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="whitespace-nowrap">{children}</dd>
    </div>
  );
}

// A project opens on its agents over the days read: each one's newest
// changes, its last trace and how that ended, its cost against its usual, the
// alerts that watch it, and its versions side by side in a panel. Anybody on
// the project reads it.
export default async function AgentsPage({ params }: PageProps<"/[organization]/[project]">) {
  const { organization, project: slug } = await params;
  const project = await fetchProjectBySlug(slug);
  if (!project) return <ServiceUnavailable />;
  const base = projectPath(organization, project.slug);
  const [read, alerts] = await Promise.all([fetchAgents(project.id), fetchAlerts(project.id)]);
  if (read.kind !== "ok") return <ServiceUnavailable />;
  const { days, agents, changes } = read.value;
  const watched = alerts.kind === "ok" ? alerts : null;
  const everyAgents = watched ? openIncidentsOf(watched.alerts, watched.incidents, null) : 0;
  const alertsHref = `${base}/alerts`;
  const ever = agents.length === 0 ? await fetchAgents(project.id, undefined, MAX_DAYS) : null;
  const firstRun = ever?.kind === "ok" && ever.value.agents.length === 0;

  return (
    <>
      <PageHeader
        title="Agents"
        info={`Every agent that ran in the last ${days} days: its last trace, what that cost against what its traces usually cost, the alerts that watch it, and their open incidents. Its name opens its own page: what failed, what changed, its traces charted and its sessions. Under its name, the newest release and first tool call marked in those days, each opening its trace, and the models and prompt versions its traces carried. Tool names are learned unmarked for its first 24 hours in an environment. Versions sets its releases, models and prompt versions side by side.`}
      />
      <div className="grid gap-page">
        {firstRun ? (
          <TraceFirstRun base={base} canMint={administers(project.role)} kind="agents" />
        ) : (
        <section className="grid gap-gutter">
        {everyAgents > 0 && (
          <NoticeCard
            title={`${counted(everyAgents, "incident is", "incidents are")} open on alerts over every agent.`}
            body={
              <Link href={alertsHref} className="underline-offset-4 hover:underline">
                Go to Alerts
              </Link>
            }
          />
        )}
        {agents.length === 0 ? (
          <NoticeCard
            title={`No agent has run here in the last ${days} days.`}
            body={
              <>
                Make an ingest key on{" "}
                <Link href={`${base}/api-keys`} className="underline underline-offset-4">
                  API keys
                </Link>{" "}
                and send a trace from an{" "}
                <Link href={`${DOCS_URL}/api/sdks`} className="underline underline-offset-4">
                  SDK
                </Link>{" "}
                or a{" "}
                <Link href={`${DOCS_URL}/api/coding-agents`} className="underline underline-offset-4">
                  coding-agent plugin
                </Link>
                : each agent then stands here with its last trace, what it cost and the alerts that watch it.
              </>
            }
          />
        ) : (
          <VersionsHost projectId={project.id} days={days}>
            <ul className="grid gap-page lg:grid-cols-2" aria-label="Agents">
              {agents.map((agent) => {
                const watchers = watched ? watchersOf(watched.alerts, agent.agent) : null;
                const open = watched ? openIncidentsOf(watched.alerts, watched.incidents, agent.agent) : 0;
                const changed = changes
                  .filter((change) => change.agent === agent.agent)
                  .toSorted((a, b) => KINDS.indexOf(a.kind) - KINDS.indexOf(b.kind));
                return (
                  <li key={agent.agent} className="min-w-0">
                    <Card className="h-full">
                      <div className="flex items-start justify-between gap-gutter">
                        <div className="grid min-w-0">
                          <Link
                            href={agentHref(organization, project.slug, agent.agent, days)}
                            className="truncate font-medium underline-offset-4 hover:underline"
                          >
                            {agent.agent}
                          </Link>
                          {changed.map((change) => (
                            <Link
                              key={change.kind}
                              href={traceHref(organization, project.slug, change.trace_id)}
                              title={changeWords(change)}
                              className="truncate text-xs text-muted-foreground underline-offset-4 hover:underline"
                            >
                              {changeWords(change)}
                            </Link>
                          ))}
                          {(
                            [
                              ["model", "models", agent.models],
                              ["prompt", "prompts", agent.prompt_versions],
                            ] as const
                          ).map(([one, many, values]) =>
                            values.length > 0 ? (
                              <span key={one} title={values.join(", ")} className="truncate text-xs text-muted-foreground">
                                {listWords(one, many, values)}
                              </span>
                            ) : null,
                          )}
                        </div>
                        <VersionsOpener agent={agent.agent} />
                      </div>
                      <dl className="grid grid-cols-2 gap-gutter text-sm">
                        <Stat label="Last trace">
                          <span className="flex items-center gap-gutter">
                            <StatusMark status={agent.last_ended ? agent.last_status : null} />
                            <Link
                              href={traceHref(organization, project.slug, agent.last_trace_id)}
                              className="underline-offset-4 hover:underline"
                            >
                              <LocalTime iso={agent.last_started_at} />
                            </Link>
                          </span>
                        </Stat>
                        <Stat label="Traces">
                          <span className="tabular-nums">{formatCount(agent.traces)}</span>
                          {agent.failed_traces > 0 && (
                            <span className="block text-xs text-destructive">{formatCount(agent.failed_traces)} failed</span>
                          )}
                        </Stat>
                        <Stat label="Last trace’s cost">
                          <span className="tabular-nums">{formatCost(agent.last_spend)}</span>
                          <span className="block text-xs text-muted-foreground">
                            {agent.usual_spend === null ? "No trace ended yet" : `Usually ${formatCost(agent.usual_spend)}`}
                          </span>
                        </Stat>
                        <Stat label="Watched by">
                          {watchers === null ? (
                            "—"
                          ) : watchers === 0 ? (
                            <Link href={alertsHref} className="text-muted-foreground underline-offset-4 hover:underline">
                              No alert yet
                            </Link>
                          ) : (
                            <Link href={alertsHref} className="underline-offset-4 hover:underline">
                              {counted(watchers, "alert", "alerts")}
                            </Link>
                          )}
                          {open > 0 && (
                            <span className="block text-xs text-destructive">{counted(open, "open incident", "open incidents")}</span>
                          )}
                        </Stat>
                      </dl>
                    </Card>
                  </li>
                );
              })}
            </ul>
          </VersionsHost>
        )}
        </section>
        )}
      </div>
    </>
  );
}
