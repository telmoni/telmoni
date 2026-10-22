import type { Metadata } from "next";
import Link from "next/link";
import { notFound } from "next/navigation";
import type { ReactNode } from "react";

import { AccessDenied } from "@/components/access-denied";
import { LocalTime } from "@/components/local-time";
import { MonitorActions } from "@/components/monitors/monitor-actions";
import { ReportLines } from "@/components/monitors/report-lines";
import { RunStrip } from "@/components/monitors/run-strip";
import { MonitorStateBadge } from "@/components/monitors/state-badge";
import { PageHeader } from "@/components/page-header";
import { Row, Rows } from "@/components/rows";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { TracePeek } from "@/components/traces/peek";
import { TraceTable } from "@/components/traces/table";
import { Card } from "@/components/ui/card";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ownerContact } from "@/lib/identity";
import {
  DETAIL_LABEL,
  RESOLUTION_LABEL,
  boundSources,
  boundValue,
  changeLine,
  formatSpend,
  monitorsHref,
  scheduleInWords,
  secondsInWords,
  stood,
  type BoundKey,
} from "@/lib/monitors";
import { administersProject } from "@/lib/project-role";
import {
  activeOrganization,
  fetchMonitor,
  fetchProjectBySlug,
  fetchResourceAudit,
  fetchTimezones,
  fetchTraces,
  getServerContext,
  type MonitorDetail,
  type OpenRun,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import { formatCount, tracesHref } from "@/lib/traces";
import { MonitorState } from "@/lib/types/enums";

/** How many of its runs the page draws: the strip's bars and the table's rows. */
const RUNS_SHOWN = 50;
/** How far back its runs are read: the widest window the trace list offers. */
const RUNS_DAYS = 90;
/** How many of its changes the page lists. */
const CHANGES_SHOWN = 20;

const BOUND_LABEL: Record<BoundKey, string> = {
  stall_after_s: "Stall timeout",
  max_steps: "Step cap",
  max_cost_usd: "Cost cap",
};

export async function generateMetadata({
  params,
}: {
  params: Promise<{ project: string; monitor: string }>;
}): Promise<Metadata> {
  const { project: slug, monitor } = await params;
  const project = await fetchProjectBySlug(slug);
  const read = project ? await fetchMonitor(project.id, monitor) : null;
  return { title: read?.kind === "ok" ? read.monitor.name : "Monitor" };
}

// One monitor: its state, when its runs are expected and its last success;
// its run going now against each bound, read from Postgres and drawn again on
// the project's `monitors:changed` nudge, never on a timer; its last fifty
// runs as a strip and as the trace list's own table, its agent's filter
// fixed; its incidents; its definition, each bound with where it came from;
// and, for the owners and admins who read the audit log, its recent changes.
export default async function MonitorPage({
  params,
}: {
  params: Promise<{ organization: string; project: string; monitor: string }>;
}) {
  const { organization, project: slug, monitor: monitorSlug } = await params;
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;
  const read = await fetchMonitor(project.id, monitorSlug);
  if (read.kind === "missing") notFound();
  if (read.kind === "unavailable") return <ServiceUnavailable />;
  const list = monitorsHref(organization, slug, {});
  if (read.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's monitors"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }
  const monitor = read.monitor;
  const canManage = administersProject(project.role);
  const runsQuery = { agent: [monitor.agent], days: RUNS_DAYS, limit: RUNS_SHOWN };
  const [runs, zones, changes] = await Promise.all([
    fetchTraces(project.id, runsQuery),
    canManage ? fetchTimezones(project.id) : null,
    canManage ? fetchResourceAudit(project.id, "monitor", monitor.id, CHANGES_SHOWN) : null,
  ]);
  const running = monitor.open_runs[0];

  return (
    <TooltipProvider>
      <PageHeader
        title={monitor.name}
        backHref={list}
        backLabel="Monitors"
        action={
          canManage ? (
            <MonitorActions
              organization={organization}
              slug={slug}
              projectId={project.id}
              zones={zones}
              monitor={monitor}
              listHref={list}
            />
          ) : undefined
        }
      />
      <div className="grid gap-6">
        <Rows>
          <Row
            label={<MonitorStateBadge state={monitor.state} />}
            hint={
              <>
                Since <LocalTime iso={monitor.state_changed_at} mode="datetime" />
              </>
            }
          >
            <span className="text-muted-foreground">{scheduleInWords(monitor)}</span>
          </Row>
          <Row label="Next due">
            <span className="text-muted-foreground">
              {monitor.next_due_at ? (
                <LocalTime iso={monitor.next_due_at} mode="datetime" />
              ) : monitor.state === MonitorState.Paused || monitor.state === MonitorState.New ? (
                "Not while paused"
              ) : (
                "No run is expected"
              )}
            </span>
          </Row>
          <Row label="Last success">
            <span className="text-muted-foreground">
              {monitor.last_success_at ? (
                <LocalTime iso={monitor.last_success_at} mode="datetime" />
              ) : (
                "None yet"
              )}
            </span>
          </Row>
        </Rows>

        {monitor.state === MonitorState.New && (
          <Card className="gap-1 p-4 text-sm">
            <p className="font-medium">Made by its first ping or run, and paused until it is set up.</p>
            <p className="text-muted-foreground">
              Its runs are kept and counted meanwhile; save its schedule and bounds with Edit to
              start watching it.
            </p>
          </Card>
        )}

        {monitor.open_runs.length > 0 && (
          <Section title="Running now">
            {monitor.open_runs.map((run) => (
              <OpenRunCard key={run.run_id} run={run} />
            ))}
          </Section>
        )}

        <Section
          title="Last runs"
          description={
            <>
              Its last {RUNS_SHOWN} runs in the last {RUNS_DAYS} days, each as long as it ran and
              coloured as it ended
              {monitor.expected_duration_s !== null
                ? `, its expected ${secondsInWords(monitor.expected_duration_s)} the line across`
                : ""}
              .{" "}
              <Link
                href={tracesHref(organization, slug, { agent: [monitor.agent], days: RUNS_DAYS })}
                className="underline underline-offset-4"
              >
                Every trace of its agent
              </Link>
            </>
          }
        >
          {runs.kind !== "ok" ? (
            <Note>Its runs could not be read. Try again in a moment.</Note>
          ) : runs.list.traces.length === 0 ? (
            <Note>No run of its agent yet. Report one, and it lands here within seconds.</Note>
          ) : (
            <>
              <RunStrip
                runs={runs.list.traces}
                expectedMs={
                  monitor.expected_duration_s === null ? null : monitor.expected_duration_s * 1_000
                }
              />
              <TraceTable
                organization={organization}
                slug={slug}
                query={runsQuery}
                ui={{}}
                traces={runs.list.traces}
              />
            </>
          )}
        </Section>

        <Section title="Incidents" description="The open one first, then the last ten resolved.">
          {monitor.incidents.length === 0 ? (
            <Note>None: nothing it watches has needed anyone.</Note>
          ) : (
            <Rows>
              {monitor.incidents.map((incident) => (
                <Row
                  key={incident.id}
                  label={
                    <span className="flex flex-wrap items-center gap-2">
                      <MonitorStateBadge state={incident.cause} />
                      <span className="font-normal">{DETAIL_LABEL[incident.detail]}</span>
                      {incident.flapping && (
                        <span className="text-xs text-muted-foreground">flapping</span>
                      )}
                    </span>
                  }
                  hint={
                    <>
                      Opened <LocalTime iso={incident.opened_at} mode="datetime" />
                      {incident.resolved_at && incident.resolution ? (
                        <>
                          {" · "}
                          {RESOLUTION_LABEL[incident.resolution]}{" "}
                          <LocalTime iso={incident.resolved_at} mode="datetime" />
                        </>
                      ) : null}
                    </>
                  }
                >
                  <span className="text-muted-foreground">
                    {incident.resolved_at
                      ? `Stood ${stood({ ...incident, resolved_at: incident.resolved_at })}`
                      : "Open"}
                  </span>
                </Row>
              ))}
            </Rows>
          )}
        </Section>

        <Section
          title="Definition"
          description="A run is held to the tighter of each bound set here and one its own code sets."
        >
          <Definition monitor={monitor} running={running} />
        </Section>

        <Section title="Report its runs">
          <Card className="p-4">
            <ReportLines slug={monitor.slug} agent={monitor.agent} />
          </Card>
        </Section>

        {changes && (
          <Section title="Recent changes">
            {changes.kind !== "ok" ? (
              <Note>Its changes could not be read. Try again in a moment.</Note>
            ) : changes.events.length === 0 ? (
              <Note>None recorded.</Note>
            ) : (
              <Rows>
                {changes.events.map((event) => (
                  <Row
                    key={event.id}
                    label={changeLine(event)}
                    hint={<span className="font-mono text-xs">{event.actor_id}</span>}
                  >
                    <span className="text-muted-foreground">
                      <LocalTime iso={event.created_at} mode="datetime" />
                    </span>
                  </Row>
                ))}
              </Rows>
            )}
          </Section>
        )}
      </div>
      <TracePeek
        organization={organization}
        slug={slug}
        projectId={project.id}
        runs={runs.kind === "ok" ? runs.list.traces.map((t) => t.run_id) : []}
      />
    </TooltipProvider>
  );
}

/** A run going now, its counts against each bound it is held to. */
function OpenRunCard({ run }: { run: OpenRun }) {
  const against = (value: string, cap: string | null) => (cap ? `${value} of ${cap}` : value);
  return (
    <Rows>
      <Row
        label={run.source === "ping" ? "Reported by its pings" : "Reported by its SDK"}
        hint={
          <>
            Started <LocalTime iso={run.started_at} mode="datetime" />, last heard{" "}
            <LocalTime iso={run.last_heard_at} mode="datetime" />
          </>
        }
      >
        <span className="text-muted-foreground">
          Over by <LocalTime iso={run.ends_by} mode="datetime" />
        </span>
      </Row>
      <Row label="Steps">
        <span className="text-muted-foreground">
          {against(
            formatCount(run.steps),
            run.bounds.max_steps === null ? null : formatCount(run.bounds.max_steps),
          )}
        </span>
      </Row>
      <Row label="Tokens">
        <span className="text-muted-foreground">
          {formatCount(run.input_tokens)} in · {formatCount(run.output_tokens)} out
        </span>
      </Row>
      <Row label="Spend">
        <span className="text-muted-foreground">
          {against(
            formatSpend(run.cost_usd),
            run.bounds.max_cost_usd === null ? null : formatSpend(run.bounds.max_cost_usd),
          )}
        </span>
      </Row>
      {run.bounds.stall_after_s !== null && (
        <Row label="Stall timeout">
          <span className="text-muted-foreground">
            Quiet for up to {secondsInWords(run.bounds.stall_after_s)}
          </span>
        </Row>
      )}
    </Rows>
  );
}

/** When its runs are expected, and each bound with where it came from. */
function Definition({ monitor, running }: { monitor: MonitorDetail; running?: OpenRun }) {
  const rows: [string, ReactNode][] = [
    ["Schedule", scheduleInWords(monitor)],
    ["Timezone", monitor.schedule === null ? "Not used" : monitor.timezone],
    ["Grace period", monitor.grace_s === null ? "None" : secondsInWords(monitor.grace_s)],
    [
      "Expected duration",
      monitor.expected_duration_s === null ? "Not set" : secondsInWords(monitor.expected_duration_s),
    ],
  ];
  for (const bound of boundSources(monitor.bounds, running?.bounds)) {
    const value =
      bound.held === null
        ? "None"
        : `${boundValue(bound.key, bound.held)}, ${
            bound.from === "code" ? "from its running run's code" : "from the monitor"
          }`;
    rows.push([BOUND_LABEL[bound.key], value]);
  }
  return (
    <Rows>
      {rows.map(([label, value]) => (
        <Row key={label} label={label}>
          <span className="text-muted-foreground">{value}</span>
        </Row>
      ))}
    </Rows>
  );
}

function Note({ children }: { children: ReactNode }) {
  return <Card className="gap-0 p-4 text-sm text-muted-foreground">{children}</Card>;
}
