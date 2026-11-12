import { Contact } from "lucide-react";
import Link from "next/link";

import { TraceFirstRun } from "@/components/first-run";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { ListPage } from "@/components/traces/list-page";
import { UserTable } from "@/components/traces/session-table";
import { ListToolbar } from "@/components/traces/toolbar";
import { MAX_DAYS } from "@/lib/list-query";
import { administers } from "@/lib/roles";
import { fetchTraces } from "@/lib/server/data";
import { fetchUserFacets, fetchUsers } from "@/lib/server/entities/users";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";
import { DEFAULT_ROWS, activeFilters, counted } from "@/lib/traces";
import { parseUserQuery } from "@/lib/users";

export const metadata = { title: "Users" };

// The users list, laid out as the trace list is, all of it in the URL: a row
// per end user in a window of days, admitted by any of their traces the
// filters admit and summed over all of them. A user opens their own page.
// Nobody here is named: an id is shown as stored, hashed.
export default async function UsersPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/users">) {
  const { organization, project: slug } = await params;
  const query = parseUserQuery(await searchParams);
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  const [access, facets] = await Promise.all([
    fetchUsers(project.id, { ...query, limit: query.limit ?? DEFAULT_ROWS }),
    fetchUserFacets(project.id, query),
  ]);
  // No user listed and no filter in force: a first run only where the project
  // has sent no trace at all. Most projects send traces that name no user, so
  // that one is told how to name one, by a read of the window's traces and
  // never of all the plan keeps, which every redraw would pay for.
  const none = access.kind === "ok" && access.list.total === 0 && activeFilters(query).length === 0;
  const sent = none ? await fetchTraces(project.id, { days: query.days, limit: 1 }) : null;
  const ever =
    sent?.kind === "ok" && sent.list.total === 0
      ? await fetchTraces(project.id, { days: MAX_DAYS, limit: 1 })
      : null;
  const firstRun = ever?.kind === "ok" && ever.list.total === 0;

  return (
    <ListPage
      list="users"
      title="Users"
      info="The people this project's agents worked for, the one last heard from first: their agents, sessions and traces, those that failed, and what they cost. A user is the traces that share the id your application gave them, which is stored hashed; the project's own people are its Members. Click a user to open their page, a value to narrow the list to it, a heading to sort by it."
      icon={Contact}
      organization={organization}
      slug={slug}
      projectId={project.id}
      gate={gate}
      query={query}
      access={access}
      facets={facets.kind === "ok" ? facets.facets : null}
      rows={(list) => list.users.length}
      toolbar={(list, days) => (
        <ListToolbar
          list="users"
          days={days}
          organization={organization}
          slug={slug}
          query={query}
          total={list.total}
        />
      )}
      firstRun={
        firstRun ? (
          <TraceFirstRun base={projectPath(organization, slug)} canMint={administers(project.role)} kind="users" />
        ) : undefined
      }
      nothingYet={(days) => (
        <>
          No trace from the last {counted(days, "day", "days")} named the user it was for. Name one when you
          open a trace — <code>user_id</code> in the{" "}
          <Link href="/docs/api/sdks" className="underline underline-offset-4">
            SDKs
          </Link>
          , <code>user.id</code> on the span — and the traces that share it list here as one user.
        </>
      )}
      table={(list) => <UserTable organization={organization} slug={slug} query={query} users={list.users} />}
    />
  );
}
