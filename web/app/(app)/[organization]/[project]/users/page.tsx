import Link from "next/link";

import { ServiceUnavailable } from "@/components/service-unavailable";
import { ListPage } from "@/components/traces/list-page";
import { ListToolbar } from "@/components/traces/toolbar";
import { UserTable } from "@/components/traces/user-table";
import { fetchUserFacets, fetchUsers } from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { DEFAULT_ROWS } from "@/lib/traces";
import { parseUserQuery } from "@/lib/users";

export const metadata = { title: "Users" };

// The users list, laid out as the trace list is, all of it in the URL: a row
// per end user in a window of days, by the id as stored — hashed unless the
// project keeps ids plain — admitted by any of their runs the filters admit.
// A user opens their traces.
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

  return (
    <ListPage
      list="users"
      title="Users"
      info="Everyone this project's agents served, by the id you tag runs with, with their traces, errors, steps and tokens. Click a user to see their traces together, a value in the filters to narrow the list to it, a heading to sort by it."
      organization={organization}
      slug={slug}
      gate={gate}
      query={query}
      access={access}
      facets={facets.kind === "ok" ? facets.facets : null}
      rows={(list) => list.users.length}
      toolbar={(list) => (
        <ListToolbar
          list="users"
          organization={organization}
          slug={slug}
          query={query}
          total={list.total}
        />
      )}
      nothingYet={(days) => (
        <>
          No run from the last {days} {days === 1 ? "day" : "days"} named the user it served.
          Name one when you open a run — <code>user_id</code> in the{" "}
          <Link href="/docs/api/sdks" className="underline underline-offset-4">
            SDKs
          </Link>
          , <code>user.id</code> on the span — and every user lists here, hashed under the
          project&rsquo;s own key unless the project keeps ids plain.
        </>
      )}
      table={(list) => (
        <UserTable organization={organization} slug={slug} query={query} users={list.users} />
      )}
    />
  );
}
