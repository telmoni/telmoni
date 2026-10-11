import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { DataTable, THead, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { SortHeading } from "@/components/sort-heading";
import { Card } from "@/components/ui/card";
import { WindowForm } from "@/components/window-form";
import { ownerContact } from "@/lib/identity";
import { MAX_LIMIT, type SearchParams } from "@/lib/list-query";
import {
  activeOrganization,
  fetchProjectBySlug,
  fetchUsers,
  getServerContext,
  type UserQuery,
  type UserRow,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import { formatCount, tracesHref } from "@/lib/traces";
import { USER_COLUMNS, currentUserSort, nextUserSort, parseUserQuery, usersHref } from "@/lib/users";

export const metadata = { title: "Users" };

// The users list: a row per end user this project's agents served in a
// window of days, by the id the application tagged its runs with, as stored
// — hashed unless the project keeps ids plain, so the page names nobody the
// project did not choose to name — with what their runs added up to. Every
// column is a sort, carried in the URL, and a user opens their traces.
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

  const access = await fetchUsers(project.id, query);
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
  const { sort, dir } = currentUserSort(query);
  const href = (patch: Partial<UserQuery>) => usersHref(organization, slug, query, patch);
  const days = query.days ?? list.days;

  return (
    <>
      <PageHeader title="Users" count={list.more ? undefined : list.users.length} />
      <div className="grid gap-6">
        <section className="grid gap-3">
          <p className="text-sm text-muted-foreground">
            Everyone this project&rsquo;s agents served, by the id you tag runs with, with their
            traces, errors, steps and tokens. Click a user to see their traces together; click a
            heading to sort by it.
          </p>

          <WindowForm
            action={projectPath(organization, slug, "/users")}
            days={days}
            carry={{ sort: query.sort, dir: query.dir }}
            label="Choose the window"
          />

          {list.users.length === 0 ? (
            <Card className="overflow-hidden p-0 gap-0">
              <div className="grid gap-1 p-4">
                <p className="text-sm font-medium">Nothing to show yet.</p>
                <p className="text-sm text-muted-foreground">
                  No run from the last {days} {days === 1 ? "day" : "days"} named the user it
                  served. Name one when you open a run — <code>user_id</code> in the{" "}
                  <Link href="/docs/api/sdks" className="underline underline-offset-4">
                    SDKs
                  </Link>
                  , <code>user.id</code> on the span — and every user lists here, hashed under
                  the project&rsquo;s own key unless the project keeps ids plain.
                </p>
              </div>
            </Card>
          ) : (
            <DataTable>
              <THead>
                {USER_COLUMNS.map((column) => (
                  <SortHeading
                    key={column.sort}
                    label={column.label}
                    column={column.sort}
                    sort={sort}
                    dir={dir}
                    href={href(nextUserSort(query, column.sort))}
                  />
                ))}
              </THead>
              <tbody>
                {list.users.map((u) => (
                  <UserTableRow
                    key={u.user_id}
                    user={u}
                    traces={tracesHref(organization, slug, { user: u.user_id })}
                  />
                ))}
              </tbody>
            </DataTable>
          )}

          {list.more && (
            <p className="text-sm text-muted-foreground">
              The first {formatCount(list.limit)} of more in this window.{" "}
              {list.limit < MAX_LIMIT ? (
                <>
                  <Link href={href({ limit: MAX_LIMIT })} className="underline underline-offset-4">
                    Show up to {formatCount(MAX_LIMIT)}
                  </Link>
                  , or narrow the window.
                </>
              ) : (
                "Narrow the window to see the rest."
              )}
            </p>
          )}
        </section>
      </div>
    </>
  );
}

function UserTableRow({ user, traces }: { user: UserRow; traces: string }) {
  return (
    <tr>
      <Td className="max-w-64 truncate font-mono text-xs" title={user.user_id}>
        <Link href={traces} className="underline-offset-4 hover:underline">
          {user.user_id}
        </Link>
      </Td>
      <Td className="whitespace-nowrap text-right tabular-nums">{formatCount(user.traces)}</Td>
      <Td
        className={`whitespace-nowrap text-right tabular-nums ${user.errors > 0 ? "text-destructive" : "text-muted-foreground"}`}
      >
        {formatCount(user.errors)}
      </Td>
      <Td className="whitespace-nowrap text-right tabular-nums">{formatCount(user.steps)}</Td>
      <Td
        className="whitespace-nowrap text-right tabular-nums"
        title={`${formatCount(user.input_tokens)} in, ${formatCount(user.output_tokens)} out`}
      >
        {formatCount(user.input_tokens + user.output_tokens)}
      </Td>
      <Td className="whitespace-nowrap text-xs text-muted-foreground">
        <LocalTime iso={user.first_seen} />
      </Td>
      <Td className="whitespace-nowrap text-xs text-muted-foreground">
        <LocalTime iso={user.last_seen} />
      </Td>
    </tr>
  );
}
