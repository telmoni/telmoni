import type { Metadata } from "next";
import Link from "next/link";

import { DataTable, THead, Td, Th } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { StatusMark } from "@/components/status-badge";
import { openIncidentsOf, watchersOf } from "@/lib/agents";
import { fetchProjectBySlug } from "@/lib/server/data";
import { fetchAgents } from "@/lib/server/entities/agents";
import { fetchAlerts } from "@/lib/server/entities/alerts";
import { DOCS_URL } from "@/lib/site";
import { projectPath } from "@/lib/slug";
import { changeWords, formatCost, formatCount, peekHref, tracesHref } from "@/lib/traces";
import { ChangeKind } from "@/lib/types/enums";

import { ConnectAgentAction } from "./_connect-agent";
import { VersionsHost, VersionsOpener } from "./_versions-panel";

// The tab is titled by the project's name, so a tab among several says which.
export async function generateMetadata({
  params,
}: PageProps<"/[organization]/[project]">): Promise<Metadata> {
  const { project: slug } = await params;
  const project = await fetchProjectBySlug(slug);
  return { title: project ? project.name : "Agents" };
}

const plural = (n: number, one: string, many: string) => `${formatCount(n)} ${n === 1 ? one : many}`;

/** The order an agent's changes stand in under its name: the contract's, the release first. */
const KINDS = Object.values(ChangeKind);

// A project opens on its agents over the days read: each one's newest
// changes, its last run and how that ended, its cost against its usual, the
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

  return (
    <>
      <PageHeader
        title="Agents"
        action={<ConnectAgentAction />}
        info={`Every agent that ran in the last ${days} days: its last run, what that cost against what its runs usually cost, the alerts that watch it, and their open incidents. Under its name, the newest release, model and prompt version it changed to in those days, each opening the run that first carried it. Versions sets its releases, models and prompt versions side by side.`}
      />
      <div className="grid gap-page">
        <section className="grid gap-gutter">
        {everyAgents > 0 && (
          <NoticeCard
            title={`${plural(everyAgents, "incident is", "incidents are")} open on alerts over every agent.`}
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
                and send a run from an{" "}
                <Link href={`${DOCS_URL}/api/sdks`} className="underline underline-offset-4">
                  SDK
                </Link>{" "}
                or a{" "}
                <Link href={`${DOCS_URL}/api/coding-agents`} className="underline underline-offset-4">
                  coding-agent plugin
                </Link>
                : each agent then stands here with its last run, what it cost and the alerts that watch it.
              </>
            }
          />
        ) : (
          <VersionsHost projectId={project.id} days={days}>
            <DataTable>
              <THead>
                <Th>Agent</Th>
                <Th>Last run</Th>
                <Th className="text-right">Runs</Th>
                <Th className="text-right">Last run&rsquo;s cost</Th>
                <Th>Watched by</Th>
                <Th />
              </THead>
              <tbody>
                {agents.map((agent) => {
                  const watchers = watched ? watchersOf(watched.alerts, agent.agent) : null;
                  const open = watched ? openIncidentsOf(watched.alerts, watched.incidents, agent.agent) : 0;
                  const changed = changes
                    .filter((change) => change.agent === agent.agent)
                    .toSorted((a, b) => KINDS.indexOf(a.kind) - KINDS.indexOf(b.kind));
                  return (
                    <tr key={agent.agent}>
                      <Td className="whitespace-nowrap">
                        <Link
                          href={tracesHref(organization, project.slug, { agent: [agent.agent] })}
                          className="font-medium underline-offset-4 hover:underline"
                        >
                          {agent.agent}
                        </Link>
                        {changed.map((change) => (
                          <Link
                            key={change.kind}
                            href={peekHref(tracesHref(organization, project.slug, { agent: [agent.agent] }), change.run_id)}
                            title={changeWords(change)}
                            className="block max-w-64 truncate text-xs text-muted-foreground underline-offset-4 hover:underline"
                          >
                            {changeWords(change)}
                          </Link>
                        ))}
                      </Td>
                      <Td className="whitespace-nowrap">
                        <span className="flex items-center gap-gutter">
                          <StatusMark status={agent.last_ended ? agent.last_status : null} />
                          <Link
                            href={peekHref(tracesHref(organization, project.slug, { agent: [agent.agent] }), agent.last_run_id)}
                            className="text-muted-foreground underline-offset-4 hover:underline"
                          >
                            <LocalTime iso={agent.last_started_at} />
                          </Link>
                        </span>
                      </Td>
                      <Td className="text-right whitespace-nowrap tabular-nums">
                        {formatCount(agent.runs)}
                        {agent.failed_runs > 0 && (
                          <span className="block text-xs text-destructive">{formatCount(agent.failed_runs)} failed</span>
                        )}
                      </Td>
                      <Td className="text-right whitespace-nowrap tabular-nums">
                        {formatCost(agent.last_spend)}
                        <span className="block text-xs text-muted-foreground">
                          {agent.usual_spend === null ? "No run ended yet" : `Usually ${formatCost(agent.usual_spend)}`}
                        </span>
                      </Td>
                      <Td className="whitespace-nowrap">
                        {watchers === null ? (
                          "—"
                        ) : watchers === 0 ? (
                          <Link href={alertsHref} className="text-muted-foreground underline-offset-4 hover:underline">
                            No alert yet
                          </Link>
                        ) : (
                          <Link href={alertsHref} className="underline-offset-4 hover:underline">
                            {plural(watchers, "alert", "alerts")}
                          </Link>
                        )}
                        {open > 0 && (
                          <span className="block text-xs text-destructive">{plural(open, "open incident", "open incidents")}</span>
                        )}
                      </Td>
                      <Td className="text-right">
                        <VersionsOpener agent={agent.agent} />
                      </Td>
                    </tr>
                  );
                })}
              </tbody>
            </DataTable>
          </VersionsHost>
        )}
        </section>
      </div>
    </>
  );
}
