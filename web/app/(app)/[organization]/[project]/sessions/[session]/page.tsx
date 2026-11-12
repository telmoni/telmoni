import type { Metadata } from "next";
import { MessagesSquare } from "lucide-react";
import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { NoticeCard } from "@/components/notice-card";
import { OverviewCard } from "@/components/overview-card";
import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { IdLink } from "@/components/traces/cells";
import { SessionCharts } from "@/components/traces/charts";
import { TracePeek } from "@/components/traces/peek";
import { SummedCards, SummedFacts, SummedNone } from "@/components/traces/summed-cards";
import { TraceTable } from "@/components/traces/table";
import { TooltipProvider } from "@/components/ui/tooltip";
import { WindowPicker } from "@/components/window-picker";
import { ownerContact } from "@/lib/identity";
import { MAX_DAYS, MAX_LIMIT, OPENING_DAYS, whole } from "@/lib/list-query";
import { administers } from "@/lib/roles";
import { activeOrganization, fetchTraces } from "@/lib/server/data";
import { fetchContentMode } from "@/lib/server/entities/content-mode";
import { fetchSessions } from "@/lib/server/entities/sessions";
import { projectPage } from "@/lib/server/project-page";
import { sessionHref, sessionsHref } from "@/lib/sessions";
import { projectPath } from "@/lib/slug";
import { fits, formatCount, formatDuration, peekHref, shortId, tracesHref } from "@/lib/traces";
import { SortDirection, TraceSort } from "@/lib/types/enums";
import { userHref } from "@/lib/users";

// The tab is titled by the session, so a tab among several says which.
export async function generateMetadata({
  params,
}: PageProps<"/[organization]/[project]/sessions/[session]">): Promise<Metadata> {
  return { title: `Session ${shortId((await params).session)}` };
}

// One session over a window of days, named by its id as stored: how it
// stands and what it took, its own charts, a step a trace, and its traces in
// the order they started, each opening beside them. Anybody on the project
// reads it. Nothing here is a message: the server sums spans.
export default async function SessionPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/sessions/[session]">) {
  const { organization, project: slug, session: id } = await params;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  // The window the URL asks for, which its links carry on; the week a list
  // opens on where it asks for none.
  const asked = whole((await searchParams).days, MAX_DAYS);
  const days = asked ?? OPENING_DAYS;
  const list = sessionsHref(organization, slug, { days: asked });
  // Oldest first, as the session ran, and as many as one read holds.
  const its = { session: [id], sort: TraceSort.Started, dir: SortDirection.Asc, days: asked };
  // An id past what one is stored as is read as one the window holds no trace of.
  const named = fits("session", id);
  const [read, traces, mode] = named
    ? await Promise.all([
        fetchSessions(project.id, { session: [id], days, limit: 1 }),
        fetchTraces(project.id, { ...its, days, limit: MAX_LIMIT }),
        fetchContentMode(project.id),
      ])
    : [null, null, null];
  if (read?.kind === "unavailable") return <ServiceUnavailable />;
  if (read?.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's sessions"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }

  const session = read?.list.sessions.at(0);
  const title = `Session ${shortId(id)}`;
  if (!session || !read) {
    return (
      <SummedNone
        title={title}
        list={{ href: list, label: "Sessions", rows: "session" }}
        icon={MessagesSquare}
        days={days}
        held={read?.list.days ?? days}
        none="No trace of this session"
      />
    );
  }

  const shown = traces?.kind === "ok" ? traces.list.traces : null;
  const total = traces?.kind === "ok" ? traces.list.total : 0;
  const here = sessionHref(organization, slug, id, asked);
  // Its last trace is the last row, unless the session holds more than a read does.
  const last = shown && total === shown.length ? shown.at(-1) : undefined;
  const elapsed = Math.max(0, Date.parse(session.last_seen) - Date.parse(session.first_seen));

  return (
    <TooltipProvider>
      <PageHeader title={title} backHref={list} backLabel="Sessions" action={<WindowPicker days={days} />} />
      <div className="grid gap-page">
        <SummedCards row={session} lastTrace={last ? peekHref(here, last.trace_id) : null}>
          {/* Not "Duration", which a chart below is headed. */}
          <OverviewCard title="Ran for">
            <p className="text-xl font-medium tabular-nums">{formatDuration(session.duration_ms)}</p>
            <p className="text-muted-foreground">
              Its traces&rsquo; own time, over {formatDuration(elapsed)} from the first to the last.
            </p>
          </OverviewCard>
        </SummedCards>

        <SummedFacts
          row={session}
          organization={organization}
          slug={slug}
          days={asked}
          tagHref={(tag) => sessionsHref(organization, slug, { tag: [tag], days: asked })}
          scores={session.scores}
        >
          {session.user_id && (
            <span className="flex items-center gap-x-gutter" title={session.user_id}>
              User
              <span className="font-medium text-foreground">
                <IdLink id={session.user_id} href={userHref(organization, slug, session.user_id, asked)} />
              </span>
            </span>
          )}
        </SummedFacts>

        <Section
          title="Over the session"
          description="A step a trace, in the order they started. Click one to open it."
        >
          {shown === null ? (
            <NoticeCard title="Its traces could not be read." body="Reload to try again." />
          ) : (
            <>
              <SessionCharts traces={shown} />
              {total > shown.length && (
                <p className="text-sm text-muted-foreground">
                  The first {formatCount(shown.length)} of its {formatCount(total)} traces are drawn.
                </p>
              )}
            </>
          )}
        </Section>

        <Section title="Traces" description="Its traces in the order they started. Click one to open it beside them.">
          {shown === null ? (
            <NoticeCard title="Its traces could not be read." body="Reload to try again." />
          ) : (
            <>
              <TraceTable organization={organization} slug={slug} query={its} traces={shown} knownTags={[]} />
              {total > shown.length && (
                <Link href={tracesHref(organization, slug, its)} className="w-fit text-sm underline underline-offset-4">
                  All {formatCount(total)} of its traces
                </Link>
              )}
            </>
          )}
        </Section>
      </div>
      {/* The trace a row or a chart's step opened, over the page: `j` and `k`
          step through its traces in their order. */}
      {shown && (
        <TracePeek
          organization={organization}
          slug={slug}
          projectId={project.id}
          traces={shown.map((trace) => trace.trace_id)}
          contentMode={mode?.content_mode}
          me={gate.person.userId}
          administers={administers(project.role)}
        />
      )}
    </TooltipProvider>
  );
}
