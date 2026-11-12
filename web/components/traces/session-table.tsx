"use client";

import type { ReactNode } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";

import { DataTable, THead, Td, Th } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { StatusMark } from "@/components/status-badge";
import { IdLink, ScoreChips, TagChips, Values } from "@/components/traces/cells";
import { ColumnHeadings, compactPadding, rowClass, rowClickTaken } from "@/components/traces/list-table";
import { scoresLine } from "@/lib/scores";
import type { Session, SessionQuery, Summed } from "@/lib/server/entities/sessions";
import type { ListQuery } from "@/lib/server/entities/traces";
import type { EndUser, UserQuery } from "@/lib/server/entities/users";
import {
  SESSIONS,
  SESSION_COLUMNS,
  SESSION_ID_COLUMN,
  SessionColumn,
  SummedColumn,
  sessionHref,
  sessionsHref,
} from "@/lib/sessions";
import { formatCost, formatCount, formatDuration, shortId, type ColumnSpec, type FilterKey } from "@/lib/traces";
import type { SessionSort, SortDirection, UserSort } from "@/lib/types/enums";
import { useTracePrefs } from "@/lib/use-trace-prefs";
import { USERS, USER_COLUMNS, USER_ID_COLUMN, UserColumn, userHref, usersHref } from "@/lib/users";
import { cn } from "@/lib/utils";

// The tables of the two lists that sum a project's traces by an id stored as
// a hash: the sessions list's and the users list's. A row's id, cut short,
// opens its own page, as a click anywhere else on the row does; a value that
// is a filter narrows its list; a heading that sorts is a link that sorts it.

/** One cell: what it shows, its look, and what it says on hover where its
 *  face says less. */
type Cell = { face: ReactNode; className: string; title?: string };

/** A count's look: right, in figures of one width. */
const NUMERIC = "whitespace-nowrap text-right tabular-nums";

/** A cell of a column both lists have, as it reads the row's sums. */
function summedCell(
  column: string,
  row: Summed,
  query: ListQuery<string>,
  narrowedTo: (filter: FilterKey, value: string) => string,
): Cell {
  switch (column) {
    case SummedColumn.Agents:
      return {
        face: <Values values={row.agents} current={query.agent} href={(agent) => narrowedTo("agent", agent)} />,
        className: "max-w-64 truncate font-medium",
        title: row.agents.join(", ") || undefined,
      };
    case SummedColumn.LastSeen:
      return {
        face: (
          <span className="flex items-center gap-gutter">
            <StatusMark status={row.last_ended ? row.last_status : null} />
            <LocalTime iso={row.last_seen} />
          </span>
        ),
        className: "whitespace-nowrap text-xs text-muted-foreground",
      };
    case SummedColumn.FirstSeen:
      return {
        face: <LocalTime iso={row.first_seen} />,
        className: "whitespace-nowrap text-xs text-muted-foreground",
      };
    case SummedColumn.Traces:
      return { face: formatCount(row.traces), className: NUMERIC };
    case SummedColumn.FailedTraces:
      return {
        face: formatCount(row.failed_traces),
        className: cn(NUMERIC, row.failed_traces > 0 ? "text-destructive" : "text-muted-foreground"),
      };
    case SummedColumn.Duration:
      return { face: formatDuration(row.duration_ms), className: NUMERIC };
    case SummedColumn.Tokens:
      return {
        face: formatCount(row.input_tokens + row.output_tokens),
        className: NUMERIC,
        title: `${formatCount(row.input_tokens)} in, ${formatCount(row.output_tokens)} out`,
      };
    case SummedColumn.Spend:
      return { face: formatCost(row.spend), className: NUMERIC };
    case SummedColumn.Tags:
      return {
        face: <TagChips tags={row.tags} current={query.tag} href={(tag) => narrowedTo("tag", tag)} />,
        className: "max-w-64 text-xs",
        title: row.tags.join(", ") || undefined,
      };
    default:
      return { face: null, className: "" };
  }
}

/** The rows of one of the two lists, under its headings. */
function SummedTable<K extends string, S extends string, R extends Summed>({
  heading,
  columns,
  sort,
  dir,
  sortHref,
  rows,
  idOf,
  pageOf,
  own,
  query,
  narrowedTo,
}: {
  /** The id column's heading. */
  heading: string;
  /** The columns the person shows, in the table's order. */
  columns: readonly ColumnSpec<K, S>[];
  sort: S;
  dir: SortDirection;
  sortHref: (column: S) => string;
  rows: readonly R[];
  idOf: (row: R) => string;
  /** The row's own page. */
  pageOf: (row: R) => string;
  /** A cell of a column this list alone has; none for one both have. */
  own: (column: K, row: R) => Cell | null;
  query: ListQuery<string>;
  narrowedTo: (filter: FilterKey, value: string) => string;
}) {
  const router = useRouter();
  const tight = compactPadding(useTracePrefs().density);
  return (
    <DataTable>
      <THead>
        <Th className="whitespace-nowrap">{heading}</Th>
        <ColumnHeadings columns={columns} sort={sort} dir={dir} sortHref={sortHref} />
      </THead>
      <tbody>
        {rows.map((row) => {
          const id = idOf(row);
          const page = pageOf(row);
          return (
            <tr
              key={id}
              // Anywhere on the row but what keeps its own way: the keyboard's
              // way is the id's link.
              onClick={(event) => {
                if (!rowClickTaken(event)) router.push(page);
              }}
              className={rowClass(false)}
            >
              <Td className={cn("whitespace-nowrap text-xs", tight)} title={id}>
                <Link href={page} className="underline-offset-4 hover:underline">
                  {shortId(id)}
                </Link>
              </Td>
              {columns.map((column) => {
                const cell = own(column.key, row) ?? summedCell(column.key, row, query, narrowedTo);
                return (
                  <Td key={column.key} className={cn(cell.className, tight)} title={cell.title}>
                    {cell.face}
                  </Td>
                );
              })}
            </tr>
          );
        })}
      </tbody>
    </DataTable>
  );
}

// The sessions list's rows, a session each: its id opens the session's page,
// its user the user's, and the columns are those the person shows
// (`lib/trace-prefs.ts`).
export function SessionTable({
  organization,
  slug,
  query,
  sessions,
}: {
  organization: string;
  slug: string;
  /** The list's query, which a heading's sort and a value's filter keep. */
  query: SessionQuery;
  sessions: Session[];
}) {
  const shown = useTracePrefs().sessionColumns;
  const { sort, dir } = SESSIONS.current(query);
  return (
    <SummedTable
      heading={SESSION_ID_COLUMN.label}
      columns={SESSION_COLUMNS.filter((c) => shown.includes(c.key))}
      sort={sort}
      dir={dir}
      sortHref={(by: SessionSort) => sessionsHref(organization, slug, query, SESSIONS.next(query, by))}
      rows={sessions}
      idOf={(session) => session.session_id}
      pageOf={(session) => sessionHref(organization, slug, session.session_id, query.days)}
      own={(column, session) => {
        switch (column) {
          case SessionColumn.User:
            return {
              face: <IdLink id={session.user_id} href={userHref(organization, slug, session.user_id, query.days)} />,
              className: "whitespace-nowrap text-xs",
              title: session.user_id || undefined,
            };
          case SessionColumn.Scores:
            return {
              face: <ScoreChips scores={session.scores} />,
              className: "max-w-64 text-xs",
              title: scoresLine(session.scores),
            };
          default:
            return null;
        }
      }}
      query={query}
      narrowedTo={(filter, value) => sessionsHref(organization, slug, query, { [filter]: [value], page: undefined })}
    />
  );
}

// The users list's rows, an end user each: their id opens the user's page,
// and how many sessions they had opens those sessions.
export function UserTable({
  organization,
  slug,
  query,
  users,
}: {
  organization: string;
  slug: string;
  /** The list's query, which a heading's sort and a value's filter keep. */
  query: UserQuery;
  users: EndUser[];
}) {
  const shown = useTracePrefs().userColumns;
  const { sort, dir } = USERS.current(query);
  return (
    <SummedTable
      heading={USER_ID_COLUMN.label}
      columns={USER_COLUMNS.filter((c) => shown.includes(c.key))}
      sort={sort}
      dir={dir}
      sortHref={(by: UserSort) => usersHref(organization, slug, query, USERS.next(query, by))}
      rows={users}
      idOf={(user) => user.user_id}
      pageOf={(user) => userHref(organization, slug, user.user_id, query.days)}
      own={(column, user) =>
        column === UserColumn.Sessions
          ? {
              face:
                user.sessions > 0 ? (
                  <Link
                    href={sessionsHref(organization, slug, { user: [user.user_id], days: query.days })}
                    className="underline-offset-4 hover:underline"
                  >
                    {formatCount(user.sessions)}
                  </Link>
                ) : (
                  formatCount(user.sessions)
                ),
              className: NUMERIC,
            }
          : null
      }
      query={query}
      narrowedTo={(filter, value) => usersHref(organization, slug, query, { [filter]: [value], page: undefined })}
    />
  );
}
