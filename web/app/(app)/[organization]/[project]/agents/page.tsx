import type { Metadata } from "next";
import { Bot } from "lucide-react";
import { cookies } from "next/headers";
import Link from "next/link";
import { redirect } from "next/navigation";

import { LocalTime } from "@/components/local-time";
import { NoticeCard } from "@/components/notice-card";
import { OverviewCard } from "@/components/overview-card";
import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { StatusMark } from "@/components/status-badge";
import { TraceCharts } from "@/components/traces/charts";
import { TracePeek } from "@/components/traces/peek";
import { SessionTable } from "@/components/traces/session-table";
import { UtcOffsetCookie } from "@/components/traces/utc-offset";
import { Button } from "@/components/ui/button";
import { TooltipProvider } from "@/components/ui/tooltip";
import { WindowPicker } from "@/components/window-picker";
import { agentHref, openIncidentsOf, watchersOf } from "@/lib/agents";
import { MAX_DAYS, OPENING_DAYS, sole, whole, windowLabel, type SearchParams } from "@/lib/list-query";
import { administers } from "@/lib/roles";
import { fetchTraceHistogram, fetchTraces } from "@/lib/server/data";
import { fetchAgents, type Agent } from "@/lib/server/entities/agents";
import { fetchAlerts } from "@/lib/server/entities/alerts";
import { fetchChanges, type Change } from "@/lib/server/entities/changes";
import { fetchContentMode } from "@/lib/server/entities/content-mode";
import { fetchSessions } from "@/lib/server/entities/sessions";
import type { Trace } from "@/lib/server/entities/traces";
import { projectPage } from "@/lib/server/project-page";
import { sessionsHref } from "@/lib/sessions";
import { projectPath } from "@/lib/slug";
import {
  FAULT_LABEL,
  changeWords,
  counted,
  fits,
  formatCost,
  formatCount,
  marksOn,
  peekHref,
  tracesHref,
} from "@/lib/traces";
import { SpanFault, SpanStatus } from "@/lib/types/enums";
import { UTC_OFFSET_COOKIE, parseUtcOffset } from "@/lib/utc-offset";
import { failedShare, formatShare } from "@/lib/versions";

import { AgentVersions } from "../_versions-panel";

/** How many of an agent's newest failed traces, changes and sessions its page names. */
const NEWEST = 5;

/** The agent the URL names, as it is named: nothing trimmed, since a name is
 *  any text; none where it names none, or one no agent could have. */
function agentOf(searchParams: SearchParams): string | undefined {
  const agent = sole(searchParams.agent);
  return agent !== undefined && fits("agent", agent) ? agent : undefined;
}

// The tab is titled by the agent, so a tab among several says which.
export async function generateMetadata({
  searchParams,
}: PageProps<"/[organization]/[project]/agents">): Promise<Metadata> {
  return { title: agentOf(await searchParams) ?? "Agent" };
}

// One agent over a window of days: how it stands, what failed and what
// changed, its traces charted with those changes marked, its versions side
// by side, and its sessions. A trace it names opens beside the page, never
// in the trace list's place. Anybody on the project reads it.
export default async function AgentPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/agents">) {
  const { organization, project: slug } = await params;
  const given = await searchParams;
  const base = projectPath(organization, slug);
  const agent = agentOf(given);
  // The Agents page is the list: this path names one of them or nothing.
  if (agent === undefined) redirect(base);
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  // The window the URL asks for, which its links carry on; the week a list
  // opens on where it asks for none.
  const asked = whole(given.days, MAX_DAYS);
  const days = asked ?? OPENING_DAYS;
  const utcOffset = parseUtcOffset((await cookies()).get(UTC_OFFSET_COOKIE)?.value);
  const its = { agent: [agent], days: asked };
  // Where a trace the page names opens: over this page, in the peek.
  const opened = (traceId: string) => peekHref(agentHref(organization, slug, agent, asked), traceId);
  const [read, alerts, histogram, changes, failed, sessions, mode] = await Promise.all([
    fetchAgents(project.id, agent, days),
    fetchAlerts(project.id),
    fetchTraceHistogram(project.id, { ...its, days }, utcOffset),
    fetchChanges(project.id, days, [agent], []),
    fetchTraces(project.id, { ...its, days, status: [SpanStatus.Error], limit: NEWEST }),
    fetchSessions(project.id, { ...its, days, limit: NEWEST }),
    // Said on an opened trace's line, the peek's.
    fetchContentMode(project.id),
  ]);
  if (read.kind !== "ok") return <ServiceUnavailable />;
  const row = read.value.agents.at(0);
  // The days the read answered: those asked, held to what the plan and the project keep.
  const held = read.value.days;
  const header = (
    <PageHeader
      title={agent}
      backHref={base}
      backLabel="Agents"
      // One visit an agent whatever window it was read over, and none of a
      // name no trace carries.
      visitHref={row ? agentHref(organization, slug, agent) : null}
      action={
        <>
          <WindowPicker days={days} />
          <Button asChild variant="outline">
            <Link href={tracesHref(organization, slug, its)}>Its traces</Link>
          </Button>
        </>
      }
    />
  );
  if (!row) {
    return (
      <>
        {header}
        <NoticeCard
          icon={Bot}
          title={`No trace of this agent in the ${windowLabel(held).toLowerCase()}.`}
          body={
            <>
              Widen the window, or go back to{" "}
              <Link href={base} className="underline underline-offset-4">
                Agents
              </Link>
              , which lists every agent that ran.
            </>
          }
        />
      </>
    );
  }

  const watched = alerts.kind === "ok" ? alerts : null;
  const watchers = watched ? watchersOf(watched.alerts, agent) : null;
  const open = watched ? openIncidentsOf(watched.alerts, watched.incidents, agent) : 0;
  const share = failedShare(row);
  const alertsHref = `${base}/alerts`;
  const marks =
    histogram.kind === "ok" && changes.kind === "ok" ? marksOn(histogram.histogram.bars, changes.value.changes) : [];

  return (
    <TooltipProvider>
      <UtcOffsetCookie drawnWith={utcOffset} />
      {header}
      <div className="grid gap-page">
        <div className="grid auto-rows-fr gap-page sm:grid-cols-2 lg:grid-cols-4">
          <OverviewCard
            title="Last trace"
            links={[{ href: opened(row.last_trace_id), label: "Open it" }]}
          >
            <p className="flex items-center gap-gutter text-xl font-medium">
              <StatusMark status={row.last_ended ? row.last_status : null} />
              <LocalTime iso={row.last_started_at} />
            </p>
            <p className="text-muted-foreground">
              {row.last_ended ? `It cost ${formatCost(row.last_spend)}.` : "Still running."}
            </p>
          </OverviewCard>
          {/* Not "Traces", which the chart below is headed. */}
          <OverviewCard title="All traces" links={[{ href: tracesHref(organization, slug, its), label: "Its traces" }]}>
            <p className="text-xl font-medium tabular-nums">{formatCount(row.traces)}</p>
            <p className={row.failed_traces > 0 ? "text-destructive" : "text-muted-foreground"}>
              {row.failed_traces > 0 && share !== null
                ? `${formatCount(row.failed_traces)} ended in error, ${formatShare(share)}.`
                : "None ended in error."}
            </p>
          </OverviewCard>
          <OverviewCard title="Spend" links={[{ href: `${base}/dashboards`, label: "Spend" }]}>
            <p className="text-xl font-medium tabular-nums">{formatCost(row.spend)}</p>
            <p className="text-muted-foreground">
              {row.usual_spend === null
                ? "No trace has ended yet."
                : `A trace usually costs ${formatCost(row.usual_spend)}.`}
            </p>
          </OverviewCard>
          <OverviewCard title="Watched by" links={[{ href: alertsHref, label: "Alerts" }]}>
            <p className="text-xl font-medium tabular-nums">
              {watchers === null ? "—" : counted(watchers, "alert", "alerts")}
            </p>
            <p className={open > 0 ? "text-destructive" : "text-muted-foreground"}>
              {watchers === null
                ? "Its alerts could not be read."
                : open > 0
                  ? `${counted(open, "incident is", "incidents are")} open.`
                  : "No incident is open."}
            </p>
          </OverviewCard>
        </div>

        <div className="grid items-start gap-page lg:grid-cols-2">
          <Failures
            agent={row}
            newest={failed.kind === "ok" ? failed.list.traces : null}
            href={(narrowed) => tracesHref(organization, slug, { ...its, ...narrowed })}
            open={opened}
          />
          <Changes
            changes={changes.kind === "ok" ? changes.value.changes : null}
            more={changes.kind === "ok" && changes.value.more}
            href={opened}
          />
        </div>

        <Section
          title="Over the window"
          description="Its traces by when they started, with each change marked on the bar it fell in."
        >
          {histogram.kind === "ok" ? (
            <TraceCharts histogram={histogram.histogram} marks={marks} models={row.models} />
          ) : (
            <NoticeCard title="The charts could not be read." body="Reload to try again." />
          )}
        </Section>

        <Section
          title="Versions"
          description="Its releases, its models and each prompt's versions, side by side."
        >
          {/* Keyed by the agent: what is picked for one is not asked of the next. */}
          <AgentVersions key={agent} projectId={project.id} agent={agent} days={held} />
        </Section>

        <Section
          title="Sessions"
          description="The sessions its traces belonged to, the one last heard from first, each summed over all of its traces."
        >
          {sessions.kind !== "ok" ? (
            <NoticeCard title="Its sessions could not be read." body="Reload to try again." />
          ) : sessions.list.sessions.length === 0 ? (
            <p className="text-sm text-muted-foreground">None of its traces in the window named a session.</p>
          ) : (
            <>
              <SessionTable
                organization={organization}
                slug={slug}
                query={its}
                sessions={sessions.list.sessions}
              />
              {sessions.list.total > sessions.list.sessions.length && (
                <Link
                  href={sessionsHref(organization, slug, its)}
                  className="w-fit text-sm underline underline-offset-4"
                >
                  All {formatCount(sessions.list.total)} of its sessions
                </Link>
              )}
            </>
          )}
        </Section>
      </div>
      {/* The trace a card opened, over the page: its ↑ and ↓ step through the newest failures. */}
      <TracePeek
        organization={organization}
        slug={slug}
        projectId={project.id}
        traces={failed.kind === "ok" ? failed.list.traces.map((trace) => trace.trace_id) : []}
        contentMode={mode?.content_mode}
        me={gate.person.userId}
        administers={administers(project.role)}
      />
    </TooltipProvider>
  );
}

/** What failed: its traces under each fault they carry, each opening the
 *  traces it names, and the newest that ended in error, each opening itself. */
function Failures({
  agent,
  newest,
  href,
  open,
}: {
  agent: Agent;
  /** Its newest traces that ended in error; none where they could not be read. */
  newest: readonly Trace[] | null;
  href: (narrowed: { fault?: SpanFault[]; status?: SpanStatus[] }) => string;
  /** Where one trace opens. */
  open: (traceId: string) => string;
}) {
  const faults = Object.values(SpanFault).flatMap((fault) => {
    const traces = agent.faults[fault] ?? 0;
    return traces > 0 ? [{ fault, traces }] : [];
  });
  const failed = href({ status: [SpanStatus.Error] });
  return (
    <OverviewCard
      title="Failures"
      links={agent.failed_traces > 0 ? [{ href: failed, label: "Its traces that ended in error" }] : []}
    >
      {faults.length === 0 && agent.failed_traces === 0 ? (
        <p className="text-muted-foreground">No trace failed, and no call of one faulted.</p>
      ) : (
        <>
          {faults.length > 0 && (
            <ul className="grid gap-1" aria-label="Traces by fault">
              {faults.map(({ fault, traces }) => (
                <li key={fault} className="flex justify-between gap-gutter">
                  <Link href={href({ fault: [fault] })} className="underline-offset-4 hover:underline">
                    {FAULT_LABEL[fault]}
                  </Link>
                  <span className="text-muted-foreground tabular-nums">{counted(traces, "trace", "traces")}</span>
                </li>
              ))}
            </ul>
          )}
          {newest === null ? (
            <p className="text-muted-foreground">Its failed traces could not be read.</p>
          ) : (
            newest.length > 0 && (
              <ul className="grid gap-1" aria-label="Newest traces that ended in error">
                {newest.map((trace) => (
                  <li key={trace.trace_id} className="flex justify-between gap-gutter">
                    <Link
                      href={open(trace.trace_id)}
                      className="min-w-0 truncate text-destructive underline-offset-4 hover:underline"
                      title={trace.error_type || undefined}
                    >
                      {trace.error_type || "Error"}
                    </Link>
                    <span className="shrink-0 text-muted-foreground">
                      <LocalTime iso={trace.started_at} />
                    </span>
                  </li>
                ))}
              </ul>
            )
          )}
        </>
      )}
    </OverviewCard>
  );
}

/** What changed in the window, the newest first: each opens the trace that
 *  first carried it. */
function Changes({
  changes,
  more,
  href,
}: {
  /** None where they could not be read. */
  changes: readonly Change[] | null;
  /** Whether the window held more than were read, its oldest left out. */
  more: boolean;
  href: (traceId: string) => string;
}) {
  const earlier = (changes?.length ?? 0) - NEWEST;
  return (
    <OverviewCard title="Changes">
      {changes === null ? (
        <p className="text-muted-foreground">Its changes could not be read.</p>
      ) : changes.length === 0 ? (
        <p className="text-muted-foreground">
          No release, model, prompt version or first tool call was marked in the window.
        </p>
      ) : (
        <>
          <ul className="grid gap-1">
            {changes.slice(0, NEWEST).map((change) => (
              // Two prompts one trace first carried at one version differ by their names alone.
              <li key={`${change.trace_id} ${changeWords(change)}`} className="flex justify-between gap-gutter">
                <Link
                  href={href(change.trace_id)}
                  className="min-w-0 truncate underline-offset-4 hover:underline"
                  title={changeWords(change)}
                >
                  {changeWords(change)}
                </Link>
                <span className="shrink-0 text-muted-foreground">
                  <LocalTime iso={change.changed_at} />
                </span>
              </li>
            ))}
          </ul>
          {earlier > 0 && (
            <p className="text-muted-foreground">
              And {formatCount(earlier)} before them, marked on the charts below
              {more && "; the window's older ones are not"}.
            </p>
          )}
        </>
      )}
    </OverviewCard>
  );
}
