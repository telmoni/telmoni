import Link from "next/link";

import { FirstRun, FirstRunStep } from "@/components/first-run";
import { InstallTabs } from "@/components/install-tabs";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { ListPage } from "@/components/traces/list-page";
import { SessionPeek } from "@/components/traces/session-peek";
import { SessionTable } from "@/components/traces/session-table";
import { ListToolbar } from "@/components/traces/toolbar";
import { administers } from "@/lib/roles";
import {
  fetchSessionFacets,
  fetchSessionLayout,
  fetchSessions,
  fetchSessionViews,
  fetchTraceLayouts,
} from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { fieldSnippets } from "@/lib/first-run";
import { MAX_DAYS } from "@/lib/list-query";
import { parseSessionQuery } from "@/lib/sessions";
import { DEFAULT_ROWS, activeFilters, parseListUi } from "@/lib/traces";

export const metadata = { title: "Sessions" };

// The sessions list, laid out as the trace list is, all of it in the URL: a
// row per session in a window of days, admitted by any of its runs the
// filters admit. A session opens beside the list
// (`components/traces/session-peek.tsx`) and has a page of its own. Nothing
// here is a message: the server sums spans.
export default async function SessionsPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/sessions">) {
  const { organization, project: slug } = await params;
  const given = await searchParams;
  const query = parseSessionQuery(given);
  const ui = parseListUi(given);
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  // The layouts and the project's default, for the session the panel opens.
  const [access, facets, setting, layouts, views] = await Promise.all([
    fetchSessions(project.id, { ...query, limit: query.limit ?? DEFAULT_ROWS }),
    fetchSessionFacets(project.id, query),
    fetchSessionLayout(project.id),
    fetchTraceLayouts(project.id),
    fetchSessionViews(project.id),
  ]);
  const canSave = administers(project.role);
  // An empty, unfiltered window is a first run only where all the plan keeps is empty too.
  const ever =
    access.kind === "ok" && access.list.total === 0 && activeFilters(query).length === 0
      ? await fetchSessions(project.id, { ...parseSessionQuery({}), days: MAX_DAYS, limit: 1 })
      : null;

  return (
    <ListPage
      list="sessions"
      title="Sessions"
      info="Every session this project's agents took part in, last heard from first, with the user it was with and its traces, errors, steps and tokens. Click a session to replay its runs beside the list, a value in the filters to narrow the list to it, a heading to sort by it."
      organization={organization}
      slug={slug}
      gate={gate}
      query={query}
      ui={ui}
      access={access}
      facets={facets.kind === "ok" ? facets.facets : null}
      rows={(list) => list.sessions.length}
      toolbar={(list, days) => (
        <ListToolbar
          list="sessions"
          days={days}
          organization={organization}
          slug={slug}
          projectId={project.id}
          query={query}
          ui={ui}
          total={list.total}
          views={views.kind === "ok" ? views.views : null}
          canSave={canSave}
        />
      )}
      firstRun={
        ever?.kind === "ok" && ever.list.total === 0 ? (
          <FirstRun
            title="Group runs into sessions"
            lead="Name the session a run belongs to — a conversation, a ticket, a job — and the runs that share it group here, each session replayed turn by turn."
          >
            <FirstRunStep
              n={1}
              title="Name the session when a run opens"
              note="The same id on every run of one conversation; any string up to 200 characters."
            >
              <InstallTabs name="Session id" tabs={fieldSnippets("session")} compact />
            </FirstRunStep>
            <FirstRunStep
              n={2}
              title="Run your agent"
              note={
                <>
                  Its sessions list here as their runs land, with no reload. The{" "}
                  <Link href="/docs/monitoring/sessions" className="underline underline-offset-4">
                    Sessions guide
                  </Link>{" "}
                  shows what each one holds.
                </>
              }
            />
          </FirstRun>
        ) : undefined
      }
      nothingYet={(days) => (
        <>
          No run from the last {days} {days === 1 ? "day" : "days"} named the session it belonged
          to. Name one when you open a run — <code>session_id</code> in the{" "}
          <Link href="/docs/api/sdks" className="underline underline-offset-4">
            SDKs
          </Link>
          , <code>gen_ai.conversation.id</code> on the span — and the runs that share it group
          here as a session.
        </>
      )}
      table={(list) => (
        <SessionTable
          organization={organization}
          slug={slug}
          query={query}
          ui={ui}
          sessions={list.sessions}
          knownTags={facets.kind === "ok" ? facets.facets.tag.values.map((v) => v.value) : []}
        />
      )}
    >
      {/* The session a row opened, over the list rather than in its place:
          its ↑ and ↓ buttons step through this page's rows. */}
      {access.kind === "ok" && (
        <SessionPeek
          organization={organization}
          slug={slug}
          projectId={project.id}
          sessions={access.list.sessions.map((s) => s.session_id)}
          window={{ days: query.days }}
          layouts={layouts.kind === "ok" ? layouts.layouts : null}
          project={setting}
          canSave={canSave}
        />
      )}
    </ListPage>
  );
}
