import Link from "next/link";
import { cookies } from "next/headers";

import { AccessDenied } from "@/components/access-denied";
import { LinkPending } from "@/components/link-pending";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { TraceCharts, TraceHistogramStrip } from "@/components/traces/charts";
import { FacetsPanel } from "@/components/traces/facets";
import { FilterBar } from "@/components/traces/filter-bar";
import { ListNotice } from "@/components/traces/list-notice";
import { NewTraces } from "@/components/traces/new-traces";
import { ListPagination } from "@/components/traces/pagination";
import { TracePeek } from "@/components/traces/peek";
import { TraceTable } from "@/components/traces/table";
import { ListHeaderControls, TraceToolbar } from "@/components/traces/toolbar";
import { UtcOffsetCookie } from "@/components/traces/utc-offset";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ownerContact } from "@/lib/identity";
import { LIST_DIMS_WHILE_PENDING } from "@/lib/list-pending";
import type { SearchParams } from "@/lib/list-query";
import {
  activeOrganization,
  fetchTraceFacets,
  fetchTraceHistogram,
  fetchTraces,
  fetchTraceViews,
} from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";
import {
  DEFAULT_ROWS,
  activeFilters,
  pageCount,
  parseTraceQuery,
  parseTraceUi,
  tracesHref,
  unfiltered,
} from "@/lib/traces";
import { administersProject } from "@/lib/project-role";
import { UTC_OFFSET_COOKIE, parseUtcOffset } from "@/lib/utc-offset";
import { cn } from "@/lib/utils";

export const metadata = { title: "Traces" };

// The trace list: a toolbar, the
// filters beside the list, the runs over the window above a page of rows,
// and the way through the pages below. A row per run of this project's
// agents in a window of days, every column a sort and most a filter, all of
// it carried in the URL so a link holds the view. Every pixel is rendered
// from metadata: the server sums each run's spans and never reads a prompt.
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
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  const overview = projectPath(organization, slug);
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's traces"
        owner={ownerContact(activeOrganization(gate))}
        backHref={overview}
      />
    );
  }

  const { list } = access;
  const days = query.days ?? list.days;
  const filtered = activeFilters(query).length > 0;
  const canSave = administersProject(project.role);
  const last = pageCount(list.total, list.limit);
  const lastPage = last > 1 ? last : undefined;
  // The rows this render drew, as one token: the table is keyed by it, and
  // the new-traces bar reads a change in it as the list drawn again.
  const drawn = list.traces.map((t) => t.run_id).join();

  // What the page says in place of its rows and its charts when the window
  // holds no trace the filters admit.
  const none =
    list.total > 0 ? null : filtered ? (
      <ListNotice
        title="No traces match these filters."
        body={
          <>
            Widen the window, or{" "}
            <Link
              href={tracesHref(organization, slug, unfiltered(query), {}, ui)}
              className="underline underline-offset-4"
            >
              clear the filters
            </Link>
            .
          </>
        }
      />
    ) : (
      <ListNotice
        title="Nothing to show yet."
        body={
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
        }
      />
    );

  // `group/list`: the header's window and every control and link below it mark
  // themselves pending while the list they asked for is on its way, and the
  // list dims under them; `contents`, so the group lays nothing out itself.
  return (
    <TooltipProvider>
      <UtcOffsetCookie drawnWith={utcOffset} />
      <div className="group/list contents">
        <PageHeader
          title="Traces"
          info="Every run of this project's agents, newest first, with its steps, model and tool calls. Click a value to narrow the list to it, a heading to sort by it."
          action={<ListHeaderControls days={days} />}
        />
        <div className="grid gap-6">
          <section className="grid gap-3">
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
            <div className="flex flex-col gap-4 md:flex-row md:items-start">
              <FacetsPanel
                list="traces"
                organization={organization}
                slug={slug}
                query={query}
                ui={ui}
                facets={facets.kind === "ok" ? facets.facets : null}
              />
              <div className={cn("grid min-w-0 flex-1 gap-3", LIST_DIMS_WHILE_PENDING)}>
                <FilterBar list="traces" organization={organization} slug={slug} query={query} ui={ui} />
                {/* What landed since this render, counted rather than drawn in:
                    the rows stay under the reader's hand until they ask. */}
                <NewTraces
                  key={project.id}
                  projectId={project.id}
                  drawn={drawn}
                  empty={list.total === 0}
                />
                {none ??
                  (ui.display === "chart" ? (
                    histogram.kind === "ok" ? (
                      <TraceCharts histogram={histogram.histogram} />
                    ) : (
                      <ListNotice title="The charts could not be read." body="Try again in a moment." />
                    )
                  ) : (
                    <>
                      {histogram.kind === "ok" && (
                        <TraceHistogramStrip histogram={histogram.histogram} />
                      )}
                      {list.traces.length > 0 ? (
                        <TraceTable
                          key={drawn}
                          organization={organization}
                          slug={slug}
                          query={query}
                          ui={ui}
                          traces={list.traces}
                        />
                      ) : (
                        <ListNotice
                          title="This page is past the last one."
                          body={
                            <Link
                              href={tracesHref(organization, slug, query, { page: lastPage }, ui)}
                              className="underline underline-offset-4"
                            >
                              Go to the last page
                              <LinkPending />
                            </Link>
                          }
                        />
                      )}
                      <ListPagination
                        list="traces"
                        organization={organization}
                        slug={slug}
                        query={query}
                        ui={ui}
                        total={list.total}
                        page={list.page}
                        limit={list.limit}
                      />
                    </>
                  ))}
              </div>
            </div>
          </section>
        </div>
      </div>
      {/* The trace a row opened, over the list rather than in its place: its
          spans are read as it opens, and its ↑ and ↓ buttons, and `j` and
          `k`, step through this page's rows. */}
      <TracePeek
        organization={organization}
        slug={slug}
        projectId={project.id}
        runs={list.traces.map((t) => t.run_id)}
      />
    </TooltipProvider>
  );
}
