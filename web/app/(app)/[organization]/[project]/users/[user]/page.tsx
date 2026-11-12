import type { Metadata } from "next";
import { Contact } from "lucide-react";
import { cookies } from "next/headers";
import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { NoticeCard } from "@/components/notice-card";
import { OverviewCard } from "@/components/overview-card";
import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { TraceCharts } from "@/components/traces/charts";
import { TracePeek } from "@/components/traces/peek";
import { SessionTable } from "@/components/traces/session-table";
import { SummedCards, SummedFacts, SummedNone } from "@/components/traces/summed-cards";
import { TraceTable } from "@/components/traces/table";
import { UtcOffsetCookie } from "@/components/traces/utc-offset";
import { TooltipProvider } from "@/components/ui/tooltip";
import { WindowPicker } from "@/components/window-picker";
import { ownerContact } from "@/lib/identity";
import { MAX_DAYS, OPENING_DAYS, whole } from "@/lib/list-query";
import { administers } from "@/lib/roles";
import { activeOrganization, fetchTraceHistogram, fetchTraces } from "@/lib/server/data";
import { fetchContentMode } from "@/lib/server/entities/content-mode";
import { fetchSessions } from "@/lib/server/entities/sessions";
import { fetchUsers } from "@/lib/server/entities/users";
import { projectPage } from "@/lib/server/project-page";
import { sessionsHref } from "@/lib/sessions";
import { projectPath } from "@/lib/slug";
import { counted, fits, formatCount, peekHref, shortId, tracesHref } from "@/lib/traces";
import { userHref, usersHref } from "@/lib/users";
import { UTC_OFFSET_COOKIE, parseUtcOffset } from "@/lib/utc-offset";

/** How many of a user's newest sessions, and of their newest traces, the page names. */
const SESSIONS_SHOWN = 5;
const TRACES_SHOWN = 25;

// The tab is titled by the user, so a tab among several says which.
export async function generateMetadata({
  params,
}: PageProps<"/[organization]/[project]/users/[user]">): Promise<Metadata> {
  return { title: `User ${shortId((await params).user)}` };
}

// One end user over a window of days, named by their id as stored: how they
// stand and what serving them took, their traces charted over the window,
// their sessions, and their newest traces, each opening beside them.
// Anybody on the project reads it. Nobody is named: the id is a hash.
export default async function UserPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/users/[user]">) {
  const { organization, project: slug, user: id } = await params;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  // The window the URL asks for, which its links carry on; the week a list
  // opens on where it asks for none.
  const asked = whole((await searchParams).days, MAX_DAYS);
  const days = asked ?? OPENING_DAYS;
  const utcOffset = parseUtcOffset((await cookies()).get(UTC_OFFSET_COOKIE)?.value);
  const list = usersHref(organization, slug, { days: asked });
  const theirs = { user: [id], days: asked };
  // An id past what one is stored as is read as one the window holds no trace for.
  const named = fits("user", id);
  const [read, histogram, sessions, traces, mode] = named
    ? await Promise.all([
        fetchUsers(project.id, { ...theirs, days, limit: 1 }),
        fetchTraceHistogram(project.id, { ...theirs, days }, utcOffset),
        fetchSessions(project.id, { ...theirs, days, limit: SESSIONS_SHOWN }),
        fetchTraces(project.id, { ...theirs, days, limit: TRACES_SHOWN }),
        fetchContentMode(project.id),
      ])
    : [null, null, null, null, null];
  if (read?.kind === "unavailable") return <ServiceUnavailable />;
  if (read?.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's users"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }

  const user = read?.list.users.at(0);
  const title = `User ${shortId(id)}`;
  if (!user || !read) {
    return (
      <SummedNone
        title={title}
        list={{ href: list, label: "Users", rows: "user" }}
        icon={Contact}
        days={days}
        held={read?.list.days ?? days}
        none="No trace for this user"
      />
    );
  }

  const shown = traces?.kind === "ok" ? traces.list : null;
  const here = userHref(organization, slug, id, asked);
  const theirSessions = sessionsHref(organization, slug, theirs);
  const newest = shown?.traces.at(0);

  return (
    <TooltipProvider>
      <UtcOffsetCookie drawnWith={utcOffset} />
      <PageHeader title={title} backHref={list} backLabel="Users" action={<WindowPicker days={days} />} />
      <div className="grid gap-page">
        <SummedCards row={user} lastTrace={newest ? peekHref(here, newest.trace_id) : null}>
          <OverviewCard
            title="Sessions"
            links={user.sessions > 0 ? [{ href: theirSessions, label: "Their sessions" }] : []}
          >
            <p className="text-xl font-medium tabular-nums">{formatCount(user.sessions)}</p>
            <p className="text-muted-foreground">
              {user.sessions > 0
                ? `Over ${counted(user.traces, "trace", "traces")}.`
                : "None of their traces named a session."}
            </p>
          </OverviewCard>
        </SummedCards>

        <SummedFacts
          row={user}
          organization={organization}
          slug={slug}
          days={asked}
          tagHref={(tag) => usersHref(organization, slug, { tag: [tag], days: asked })}
        />

        <Section title="Over the window" description="Their traces by when they started.">
          {histogram?.kind === "ok" ? (
            <TraceCharts histogram={histogram.histogram} marks={[]} />
          ) : (
            <NoticeCard title="The charts could not be read." body="Reload to try again." />
          )}
        </Section>

        <Section
          title="Sessions"
          description="The sessions their traces belonged to, the one last heard from first, each summed over all of its traces."
        >
          {sessions?.kind !== "ok" ? (
            <NoticeCard title="Their sessions could not be read." body="Reload to try again." />
          ) : sessions.list.sessions.length === 0 ? (
            <p className="text-sm text-muted-foreground">None of their traces in the window named a session.</p>
          ) : (
            <>
              <SessionTable organization={organization} slug={slug} query={theirs} sessions={sessions.list.sessions} />
              {sessions.list.total > sessions.list.sessions.length && (
                <Link href={theirSessions} className="w-fit text-sm underline underline-offset-4">
                  All {formatCount(sessions.list.total)} of their sessions
                </Link>
              )}
            </>
          )}
        </Section>

        <Section title="Traces" description="Their traces, the newest first. Click one to open it beside them.">
          {shown === null ? (
            <NoticeCard title="Their traces could not be read." body="Reload to try again." />
          ) : (
            <>
              <TraceTable organization={organization} slug={slug} query={theirs} traces={shown.traces} knownTags={[]} />
              {shown.total > shown.traces.length && (
                <Link
                  href={tracesHref(organization, slug, theirs)}
                  className="w-fit text-sm underline underline-offset-4"
                >
                  All {formatCount(shown.total)} of their traces
                </Link>
              )}
            </>
          )}
        </Section>
      </div>
      {/* The trace a row opened, over the page: `j` and `k` step through the rows. */}
      {shown && (
        <TracePeek
          organization={organization}
          slug={slug}
          projectId={project.id}
          traces={shown.traces.map((trace) => trace.trace_id)}
          contentMode={mode?.content_mode}
          me={gate.person.userId}
          administers={administers(project.role)}
        />
      )}
    </TooltipProvider>
  );
}
