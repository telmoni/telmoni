import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { LinkPending } from "@/components/link-pending";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { FacetsPanel } from "@/components/traces/facets";
import { FilterBar } from "@/components/traces/filter-bar";
import { ListNotice } from "@/components/traces/list-notice";
import { ListPagination } from "@/components/traces/pagination";
import { SessionPeek } from "@/components/traces/session-peek";
import { SessionTable } from "@/components/traces/session-table";
import { ListHeaderControls, ListToolbar } from "@/components/traces/toolbar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ownerContact } from "@/lib/identity";
import { LIST_DIMS_WHILE_PENDING } from "@/lib/list-pending";
import type { SearchParams } from "@/lib/list-query";
import { administersProject } from "@/lib/project-role";
import {
  activeOrganization,
  fetchSessionFacets,
  fetchSessionLayout,
  fetchSessions,
  fetchTraceLayouts,
} from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { parseSessionQuery, sessionsHref } from "@/lib/sessions";
import { projectPath } from "@/lib/slug";
import { DEFAULT_ROWS, activeFilters, pageCount, unfiltered } from "@/lib/traces";
import { cn } from "@/lib/utils";

export const metadata = { title: "Sessions" };

// The sessions list, laid out as the trace list is, all of it in the URL: a
// row per session in a window of days, admitted by any of its runs the
// filters admit. A session opens beside the list
// (`components/traces/session-peek.tsx`) and has a page of its own. Nothing
// here is a message: the server sums spans.
export default async function SessionsPage({
  params,
  searchParams,
}: {
  params: Promise<{ organization: string; project: string }>;
  searchParams?: Promise<SearchParams>;
}) {
  const { organization, project: slug } = await params;
  const query = parseSessionQuery((await searchParams) ?? {});
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  // The layouts and the project's default, for the session the panel opens.
  const [access, facets, setting, layouts] = await Promise.all([
    fetchSessions(project.id, { ...query, limit: query.limit ?? DEFAULT_ROWS }),
    fetchSessionFacets(project.id, query),
    fetchSessionLayout(project.id),
    fetchTraceLayouts(project.id),
  ]);
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's sessions"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }

  const { list } = access;
  const days = query.days ?? list.days;
  const filtered = activeFilters(query).length > 0;
  const last = pageCount(list.total, list.limit);
  const lastPage = last > 1 ? last : undefined;

  // What the page says in place of its rows when the window holds no session
  // the filters admit.
  const none =
    list.total > 0 ? null : filtered ? (
      <ListNotice
        title="No sessions match these filters."
        body={
          <>
            Widen the window, or{" "}
            <Link
              href={sessionsHref(organization, slug, unfiltered(query))}
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
            No run from the last {days} {days === 1 ? "day" : "days"} named the session it
            belonged to. Name one when you open a run — <code>session_id</code> in the{" "}
            <Link href="/docs/api/sdks" className="underline underline-offset-4">
              SDKs
            </Link>
            , <code>gen_ai.conversation.id</code> on the span — and the runs that share it group
            here as a session.
          </>
        }
      />
    );

  // `group/list`: the header's window and every control and link below it mark
  // themselves pending while the list they asked for is on its way, and the
  // list dims under them; `contents`, so the group lays nothing out itself.
  return (
    <TooltipProvider>
      <div className="group/list contents">
        <PageHeader
          title="Sessions"
          info="Every session this project's agents took part in, last heard from first, with the user it was with and its traces, errors, steps and tokens. Click a session to replay its runs beside the list, a value in the filters to narrow the list to it, a heading to sort by it."
          action={<ListHeaderControls days={days} />}
        />
        <div className="grid gap-6">
          <section className="grid gap-3">
            <ListToolbar
              list="sessions"
              organization={organization}
              slug={slug}
              query={query}
              total={list.total}
            />
            <div className="flex flex-col gap-4 md:flex-row md:items-start">
              <FacetsPanel
                list="sessions"
                organization={organization}
                slug={slug}
                query={query}
                facets={facets.kind === "ok" ? facets.facets : null}
              />
              <div className={cn("grid min-w-0 flex-1 gap-3", LIST_DIMS_WHILE_PENDING)}>
                <FilterBar list="sessions" organization={organization} slug={slug} query={query} />
                {none ?? (
                  <>
                    {list.sessions.length > 0 ? (
                      <SessionTable
                        organization={organization}
                        slug={slug}
                        query={query}
                        sessions={list.sessions}
                      />
                    ) : (
                      <ListNotice
                        title="This page is past the last one."
                        body={
                          <Link
                            href={sessionsHref(organization, slug, query, { page: lastPage })}
                            className="underline underline-offset-4"
                          >
                            Go to the last page
                            <LinkPending />
                          </Link>
                        }
                      />
                    )}
                    <ListPagination
                      list="sessions"
                      organization={organization}
                      slug={slug}
                      query={query}
                      total={list.total}
                      page={list.page}
                      limit={list.limit}
                    />
                  </>
                )}
              </div>
            </div>
          </section>
        </div>
      </div>
      {/* The session a row opened, over the list rather than in its place:
          its ↑ and ↓ buttons step through this page's rows. */}
      <SessionPeek
        organization={organization}
        slug={slug}
        projectId={project.id}
        sessions={list.sessions.map((s) => s.session_id)}
        window={{ days: query.days }}
        layouts={layouts.kind === "ok" ? layouts.layouts : null}
        project={setting}
        canSave={administersProject(project.role)}
      />
    </TooltipProvider>
  );
}
