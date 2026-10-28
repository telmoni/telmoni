import Link from "next/link";
import type { ReactNode } from "react";

import { AccessDenied } from "@/components/access-denied";
import { LinkPending } from "@/components/link-pending";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { FacetsPanel } from "@/components/traces/facets";
import { FilterBar } from "@/components/traces/filter-bar";
import { ListNotice } from "@/components/traces/list-notice";
import { ListPagination } from "@/components/traces/pagination";
import { ListHeaderControls } from "@/components/traces/toolbar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ownerContact } from "@/lib/identity";
import { LIST_DIMS_WHILE_PENDING } from "@/lib/list-pending";
import { activeOrganization, type ServerContext } from "@/lib/server/data";
import type { Facets, ListQuery } from "@/lib/server/entities/traces";
import { projectPath } from "@/lib/slug";
import {
  activeFilters,
  listPageHref,
  pageCount,
  unfiltered,
  type ListName,
  type TraceUi,
} from "@/lib/traces";
import { cn } from "@/lib/utils";

/** What every list's page carries of its window and its pages. */
type ListWindow = { days: number; total: number; page: number; limit: number };

/** One list's page — the traces', the sessions' or the users' — in the
 *  layout the three share: the forbidden and unavailable branches, the
 *  window and the filters, what the page says when the window holds nothing
 *  or the page is past the last, the table and the way through its pages.
 *  What differs is handed in: the toolbar, the table, the words, and what the
 *  list draws beside them. A Server Component, so its render props run on
 *  the server. */
export function ListPage<L extends ListWindow>({
  list,
  title,
  info,
  organization,
  slug,
  gate,
  query,
  ui = {},
  access,
  facets,
  rows,
  toolbar,
  nothingYet,
  before,
  instead,
  table,
  children,
}: {
  list: ListName;
  title: string;
  info: string;
  organization: string;
  slug: string;
  /** What the project page resolved, for the owner a refusal names. */
  gate: ServerContext;
  query: ListQuery<string>;
  /** The trace list's own state, carried into its links. */
  ui?: TraceUi;
  access: { kind: "ok"; list: L } | { kind: "forbidden" } | { kind: "unavailable" };
  facets: Facets | null;
  /** How many rows this page holds. */
  rows: (list: L) => number;
  toolbar: (list: L) => ReactNode;
  /** What the page says when the window holds nothing at all. */
  nothingYet: (days: number) => ReactNode;
  /** Drawn under the filter bar, before the rows. */
  before?: ReactNode;
  /** Drawn in place of the table and its pages. */
  instead?: ReactNode;
  table: (list: L) => ReactNode;
  /** Drawn after the list: a row's peek. */
  children?: ReactNode;
}) {
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what={`this project's ${list}`}
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }

  const listed = access.list;
  const days = query.days ?? listed.days;
  const filtered = activeFilters(query).length > 0;
  const last = pageCount(listed.total, listed.limit);
  const lastPage = last > 1 ? last : undefined;

  // What the page says in place of its rows when the window holds nothing
  // the filters admit.
  const none =
    listed.total > 0 ? null : filtered ? (
      <ListNotice
        title={`No ${list} match these filters.`}
        body={
          <>
            Widen the window, or{" "}
            <Link
              href={listPageHref(list, organization, slug, unfiltered(query), ui)}
              className="underline underline-offset-4"
            >
              clear the filters
            </Link>
            .
          </>
        }
      />
    ) : (
      <ListNotice title="Nothing to show yet." body={nothingYet(days)} />
    );

  // `group/list`: the header's window and every control and link below it mark
  // themselves pending while the list they asked for is on its way, and the
  // list dims under them; `contents`, so the group lays nothing out itself.
  return (
    <TooltipProvider>
      <div className="group/list contents">
        <PageHeader title={title} info={info} action={<ListHeaderControls days={days} />} />
        <div className="grid gap-6">
          <section className="grid gap-3">
            {toolbar(listed)}
            <div className="flex flex-col gap-4 md:flex-row md:items-start">
              <FacetsPanel
                list={list}
                organization={organization}
                slug={slug}
                query={query}
                ui={ui}
                facets={facets}
              />
              <div className={cn("grid min-w-0 flex-1 gap-3", LIST_DIMS_WHILE_PENDING)}>
                <FilterBar list={list} organization={organization} slug={slug} query={query} ui={ui} />
                {before}
                {none ??
                  instead ?? (
                    <>
                      {rows(listed) > 0 ? (
                        table(listed)
                      ) : (
                        <ListNotice
                          title="This page is past the last one."
                          body={
                            <Link
                              href={listPageHref(
                                list,
                                organization,
                                slug,
                                { ...query, page: lastPage },
                                ui,
                              )}
                              className="underline underline-offset-4"
                            >
                              Go to the last page
                              <LinkPending />
                            </Link>
                          }
                        />
                      )}
                      <ListPagination
                        list={list}
                        organization={organization}
                        slug={slug}
                        query={query}
                        ui={ui}
                        total={listed.total}
                        page={listed.page}
                        limit={listed.limit}
                      />
                    </>
                  )}
              </div>
            </div>
          </section>
        </div>
      </div>
      {children}
    </TooltipProvider>
  );
}
