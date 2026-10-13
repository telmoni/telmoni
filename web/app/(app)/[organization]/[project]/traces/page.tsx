import Link from "next/link";
import { cookies } from "next/headers";

import { AccessDenied } from "@/components/access-denied";
import { LinkPending } from "@/components/link-pending";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { TraceCharts, TraceHistogramStrip } from "@/components/traces/charts";
import { TraceFacetsPanel } from "@/components/traces/facets";
import { TraceFilterBar } from "@/components/traces/filter-bar";
import { NewTraces } from "@/components/traces/new-traces";
import { TracePagination } from "@/components/traces/pagination";
import { TraceTable } from "@/components/traces/table";
import { TraceHeaderControls, TraceToolbar } from "@/components/traces/toolbar";
import { UtcOffsetCookie } from "@/components/traces/utc-offset";
import { Card } from "@/components/ui/card";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ownerContact } from "@/lib/identity";
import { LIST_DIMS_WHILE_PENDING } from "@/lib/list-pending";
import type { SearchParams } from "@/lib/list-query";
import {
  activeOrganization,
  fetchProjectBySlug,
  fetchTraceFacets,
  fetchTraceHistogram,
  fetchTraces,
  fetchTraceViews,
  getServerContext,
} from "@/lib/server/data";
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
import { Role } from "@/lib/types/enums";
import { UTC_OFFSET_COOKIE, parseUtcOffset } from "@/lib/utc-offset";
import { cn } from "@/lib/utils";

export const metadata = { title: "Traces" };

// The trace list, laid out as Langfuse's tracing page: a toolbar, the
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
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;

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
  const canSave = project.role === Role.Owner || project.role === Role.Admin;
  const last = pageCount(list.total, list.limit);
  const lastPage = last > 1 ? last : undefined;
  // The rows this render drew, as one token: the table is keyed by it, and
  // the new-traces bar reads a change in it as the list drawn again.
  const drawn = list.traces.map((t) => t.run_id).join();

  // What the page says in place of its rows and its charts when the window
  // holds no trace the filters admit.
  const none =
    list.total > 0 ? null : filtered ? (
      <Notice
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
      <Notice
        title="Nothing to show yet."
        body={
          <>
            Install an{" "}
            <Link href="/docs/api/sdks" className="underline underline-offset-4">
              SDK
            </Link>
            , wrap one function as a run, and its trace lists here within a minute of ending.
            Runs from the last {days} {days === 1 ? "day" : "days"} are shown.
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
          count={list.total}
          info="Every run of this project's agents, newest first, with its steps, model and tool calls. Click a value to narrow the list to it, a heading to sort by it."
          action={<TraceHeaderControls days={days} />}
        />
        <div className="grid gap-3">
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
            <TraceFacetsPanel
              organization={organization}
              slug={slug}
              query={query}
              ui={ui}
              facets={facets.kind === "ok" ? facets.facets : null}
            />
            <div className={cn("grid min-w-0 flex-1 gap-3", LIST_DIMS_WHILE_PENDING)}>
              <TraceFilterBar organization={organization} slug={slug} query={query} ui={ui} />
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
                    <Notice title="The charts could not be read." body="Try again in a moment." />
                  )
                ) : (
                  <>
                    {histogram.kind === "ok" && <TraceHistogramStrip histogram={histogram.histogram} />}
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
                      <Notice
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
                    <TracePagination
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
        </div>
      </div>
    </TooltipProvider>
  );
}

/** What the page says in the table's place. */
function Notice({ title, body }: { title: string; body: React.ReactNode }) {
  return (
    <Card className="gap-0 overflow-hidden p-0">
      <div className="grid gap-1 p-4">
        <p className="text-sm font-medium">{title}</p>
        <p className="text-sm text-muted-foreground">{body}</p>
      </div>
    </Card>
  );
}
