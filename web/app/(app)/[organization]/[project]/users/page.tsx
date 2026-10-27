import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { LinkPending } from "@/components/link-pending";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { FacetsPanel } from "@/components/traces/facets";
import { FilterBar } from "@/components/traces/filter-bar";
import { ListNotice } from "@/components/traces/list-notice";
import { ListPagination } from "@/components/traces/pagination";
import { ListHeaderControls, ListToolbar } from "@/components/traces/toolbar";
import { UserTable } from "@/components/traces/user-table";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ownerContact } from "@/lib/identity";
import { LIST_DIMS_WHILE_PENDING } from "@/lib/list-pending";
import type { SearchParams } from "@/lib/list-query";
import {
  activeOrganization,
  fetchProjectBySlug,
  fetchUserFacets,
  fetchUsers,
  getServerContext,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import { DEFAULT_ROWS, activeFilters, pageCount, unfiltered } from "@/lib/traces";
import { parseUserQuery, usersHref } from "@/lib/users";
import { cn } from "@/lib/utils";

export const metadata = { title: "Users" };

// The users list, laid out as the trace list is, all of it in the URL: a row
// per end user in a window of days, by the id as stored — hashed unless the
// project keeps ids plain — admitted by any of their runs the filters admit.
// A user opens their traces.
export default async function UsersPage({
  params,
  searchParams,
}: {
  params: Promise<{ organization: string; project: string }>;
  searchParams?: Promise<SearchParams>;
}) {
  const { organization, project: slug } = await params;
  const query = parseUserQuery((await searchParams) ?? {});
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;

  const [access, facets] = await Promise.all([
    fetchUsers(project.id, { ...query, limit: query.limit ?? DEFAULT_ROWS }),
    fetchUserFacets(project.id, query),
  ]);
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's users"
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

  // What the page says in place of its rows when the window holds no user
  // the filters admit.
  const none =
    list.total > 0 ? null : filtered ? (
      <ListNotice
        title="No users match these filters."
        body={
          <>
            Widen the window, or{" "}
            <Link
              href={usersHref(organization, slug, unfiltered(query))}
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
            No run from the last {days} {days === 1 ? "day" : "days"} named the user it served.
            Name one when you open a run — <code>user_id</code> in the{" "}
            <Link href="/docs/api/sdks" className="underline underline-offset-4">
              SDKs
            </Link>
            , <code>user.id</code> on the span — and every user lists here, hashed under the
            project&rsquo;s own key unless the project keeps ids plain.
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
          title="Users"
          info="Everyone this project's agents served, by the id you tag runs with, with their traces, errors, steps and tokens. Click a user to see their traces together, a value in the filters to narrow the list to it, a heading to sort by it."
          action={<ListHeaderControls days={days} />}
        />
        <div className="grid gap-6">
          <section className="grid gap-3">
            <ListToolbar
              list="users"
              organization={organization}
              slug={slug}
              query={query}
              total={list.total}
            />
            <div className="flex flex-col gap-4 md:flex-row md:items-start">
              <FacetsPanel
                list="users"
                organization={organization}
                slug={slug}
                query={query}
                facets={facets.kind === "ok" ? facets.facets : null}
              />
              <div className={cn("grid min-w-0 flex-1 gap-3", LIST_DIMS_WHILE_PENDING)}>
                <FilterBar list="users" organization={organization} slug={slug} query={query} />
                {none ?? (
                  <>
                    {list.users.length > 0 ? (
                      <UserTable
                        organization={organization}
                        slug={slug}
                        query={query}
                        users={list.users}
                      />
                    ) : (
                      <ListNotice
                        title="This page is past the last one."
                        body={
                          <Link
                            href={usersHref(organization, slug, query, { page: lastPage })}
                            className="underline underline-offset-4"
                          >
                            Go to the last page
                            <LinkPending />
                          </Link>
                        }
                      />
                    )}
                    <ListPagination
                      list="users"
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
    </TooltipProvider>
  );
}
