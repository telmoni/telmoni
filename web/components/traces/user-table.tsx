"use client";

import Link from "next/link";

import { DataTable, THead, Td } from "@/components/data-table";
import { SortHeading } from "@/components/sort-heading";
import { ColumnHeadings, compactPadding } from "@/components/traces/list-table";
import { sumCell, sumClass, sumTitle } from "@/components/traces/sum-cells";
import type { UserQuery, UserRow } from "@/lib/server/entities/users";
import { tracesHref, type ListUi } from "@/lib/traces";
import { UserSort } from "@/lib/types/enums";
import { useTracePrefs } from "@/lib/use-trace-prefs";
import {
  USER_COLUMNS,
  USER_ID_COLUMN,
  currentUserSort,
  nextUserSort,
  usersHref,
} from "@/lib/users";
import { cn } from "@/lib/utils";

type Props = {
  organization: string;
  slug: string;
  query: UserQuery;
  /** The page's own, which a heading's sort keeps. */
  ui: ListUi;
  users: UserRow[];
};

// The page's rows, a user each: their id, which opens their traces, and the
// columns the person shows (`lib/trace-prefs.ts`), each heading a link that
// sorts by it.
export function UserTable({ organization, slug, query, ui, users }: Props) {
  const prefs = useTracePrefs();
  const columns = USER_COLUMNS.filter((c) => prefs.userColumns.includes(c.key));
  const { sort, dir } = currentUserSort(query);
  const tight = compactPadding(prefs.density);
  const sorted = (column: UserSort) => usersHref(organization, slug, query, nextUserSort(query, column), ui);

  return (
    <DataTable>
      <THead>
        <SortHeading
          label={USER_ID_COLUMN.label}
          column={UserSort.User}
          sort={sort}
          dir={dir}
          href={sorted(UserSort.User)}
        />
        <ColumnHeadings columns={columns} sort={sort} dir={dir} sortHref={sorted} />
      </THead>
      <tbody>
        {users.map((user) => (
          <tr key={user.user_id}>
            <Td className={cn("max-w-64 truncate text-xs", tight)} title={user.user_id}>
              <Link
                href={tracesHref(organization, slug, { user: [user.user_id], days: query.days })}
                className="underline-offset-4 hover:underline"
              >
                {user.user_id}
              </Link>
            </Td>
            {columns.map((column) => (
              <Td
                key={column.key}
                className={cn(sumClass(column.key, user), tight)}
                title={sumTitle(column.key, user)}
              >
                {sumCell(column.key, user)}
              </Td>
            ))}
          </tr>
        ))}
      </tbody>
    </DataTable>
  );
}
