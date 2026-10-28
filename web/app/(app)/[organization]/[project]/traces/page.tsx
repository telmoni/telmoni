import Link from "next/link";
import { cookies } from "next/headers";

import { ServiceUnavailable } from "@/components/service-unavailable";
import { TraceCharts, TraceHistogramStrip } from "@/components/traces/charts";
import { ListNotice } from "@/components/traces/list-notice";
import { ListPage } from "@/components/traces/list-page";
import { NewTraces } from "@/components/traces/new-traces";
import { TracePeek } from "@/components/traces/peek";
import { TraceTable } from "@/components/traces/table";
import { TraceToolbar } from "@/components/traces/toolbar";
import { UtcOffsetCookie } from "@/components/traces/utc-offset";
import type { SearchParams } from "@/lib/list-query";
import { administers } from "@/lib/roles";
import {
  fetchTraceFacets,
  fetchTraceHistogram,
  fetchTraces,
  fetchTraceViews,
} from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { DEFAULT_ROWS, parseTraceQuery, parseTraceUi } from "@/lib/traces";
import { UTC_OFFSET_COOKIE, parseUtcOffset } from "@/lib/utc-offset";

export const metadata = { title: "Traces" };

// The trace list: a toolbar, the filters beside the list, the runs over the
// window above a page of rows, and the way through the pages below. A row
// per run of this project's agents in a window of days, every column a sort
// and most a filter, all of it carried in the URL so a link holds the view.
// Every pixel is rendered from metadata: the server sums each run's spans
// and never reads a prompt.
export default async function TracesPage({
  params,
  searchParams,
}: {
  params: Promise<{ organization: string; project: string }>;
  searchParams?: Promise<SearchParams>;
}) {
  const { organization, project: slug } = await params;
  const given = (await searchParams) ?? {};
  const query = parseTraceQuery(given);
  const ui = parseTraceUi(given);
  const utcOffset = parseUtcOffset((await cookies()).get(UTC_OFFSET_COOKIE)?.value);
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  const [access, facets, histogram, views] = await Promise.all([
    fetchTraces(project.id, { ...query, limit: query.limit ?? DEFAULT_ROWS }),
    fetchTraceFacets(project.id, query),
    fetchTraceHistogram(project.id, query, utcOffset),
    fetchTraceViews(project.id),
  ]);
  const canSave = administers(project.role);
  // The rows this render drew, as one token: the table is keyed by it, and
  // the new-traces bar reads a change in it as the list drawn again.
  const drawn = access.kind === "ok" ? access.list.traces.map((t) => t.run_id).join() : "";

  return (
    <>
      <UtcOffsetCookie drawnWith={utcOffset} />
      <ListPage
        list="traces"
        title="Traces"
        info="Every run of this project's agents, newest first, with its steps, model and tool calls. Click a value to narrow the list to it, a heading to sort by it."
        organization={organization}
        slug={slug}
        gate={gate}
        query={query}
        ui={ui}
        access={access}
        facets={facets.kind === "ok" ? facets.facets : null}
        rows={(list) => list.traces.length}
        toolbar={(list) => (
          <TraceToolbar
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
        nothingYet={(days) => (
          <>
            Install an{" "}
            <Link href="/docs/api/sdks" className="underline underline-offset-4">
              SDK
            </Link>{" "}
            and wrap one function as a run, or add the{" "}
            <Link href="/docs/api/coding-agents" className="underline underline-offset-4">
              coding-agent plugin
            </Link>{" "}
            for Claude Code, Codex, OpenCode or Cursor, and each run&rsquo;s trace lists here
            within a minute of ending. Runs from the last {days} {days === 1 ? "day" : "days"} are
            shown.
          </>
        )}
        // What landed since this render, counted rather than drawn in: the
        // rows stay under the reader's hand until they ask.
        before={
          access.kind === "ok" && (
            <NewTraces
              key={project.id}
              projectId={project.id}
              drawn={drawn}
              empty={access.list.total === 0}
            />
          )
        }
        // The charts in place of the table, when the page asks for them.
        instead={
          ui.display === "chart" ? (
            histogram.kind === "ok" ? (
              <TraceCharts histogram={histogram.histogram} />
            ) : (
              <ListNotice title="The charts could not be read." body="Try again in a moment." />
            )
          ) : undefined
        }
        table={(list) => (
          <>
            {histogram.kind === "ok" && <TraceHistogramStrip histogram={histogram.histogram} />}
            <TraceTable
              key={drawn}
              organization={organization}
              slug={slug}
              query={query}
              ui={ui}
              traces={list.traces}
            />
          </>
        )}
      >
        {/* The trace a row opened, over the list rather than in its place: its
            spans are read as it opens, and its ↑ and ↓ buttons, and `j` and
            `k`, step through this page's rows. */}
        {access.kind === "ok" && (
          <TracePeek
            organization={organization}
            slug={slug}
            projectId={project.id}
            runs={access.list.traces.map((t) => t.run_id)}
          />
        )}
      </ListPage>
    </>
  );
}
