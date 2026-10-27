"use client";

import Link from "next/link";

import { DataTable, THead, Td, Th } from "@/components/data-table";
import { SortHeading } from "@/components/sort-heading";
import { SessionLink } from "@/components/traces/session-link";
import { usePeekedSession } from "@/components/traces/session-peek";
import { sumCell, sumClass, sumTitle } from "@/components/traces/sum-cells";
import { openPeek } from "@/lib/peek";
import type { SessionQuery, SessionRow } from "@/lib/server/entities/sessions";
import {
  SESSION_COLUMNS,
  SESSION_ID_COLUMN,
  currentSessionSort,
  nextSessionSort,
  sessionHref,
  sessionsHref,
} from "@/lib/sessions";
import { tracesHref } from "@/lib/traces";
import { SessionSort } from "@/lib/types/enums";
import { useTracePrefs } from "@/lib/use-trace-prefs";
import { cn } from "@/lib/utils";

type Props = {
  organization: string;
  slug: string;
  query: SessionQuery;
  sessions: SessionRow[];
};

// The page's rows, a session each, in the columns the person shows: a click
// on the id or anywhere else on the row opens the session beside the list
// (`session-peek.tsx`), and the row stays marked while it is open.
export function SessionTable({ organization, slug, query, sessions }: Props) {
  const prefs = useTracePrefs();
  const peeked = usePeekedSession();
  const columns = SESSION_COLUMNS.filter((c) => prefs.sessionColumns.includes(c.key));
  const { sort, dir } = currentSessionSort(query);
  const tight = prefs.density === "compact" ? "py-1.5" : undefined;
  const sorted = (column: SessionSort) =>
    sessionsHref(organization, slug, query, nextSessionSort(query, column));

  return (
    <DataTable>
      <THead>
        <SortHeading
          label={SESSION_ID_COLUMN.label}
          column={SessionSort.Session}
          sort={sort}
          dir={dir}
          href={sorted(SessionSort.Session)}
        />
        {columns.map((column) =>
          column.sort ? (
            <SortHeading
              key={column.key}
              label={column.label}
              column={column.sort}
              sort={sort}
              dir={dir}
              href={sorted(column.sort)}
              className={column.numeric ? "text-right" : undefined}
            />
          ) : (
            <Th key={column.key} className={cn("whitespace-nowrap", column.numeric && "text-right")}>
              {column.label}
            </Th>
          ),
        )}
      </THead>
      <tbody>
        {sessions.map((session) => (
          <tr
            key={session.session_id}
            aria-current={peeked === session.session_id ? "true" : undefined}
            // Anywhere on the row but a link of its own, and but a drag that
            // chose some text to copy: the keyboard's way is the id's link.
            onClick={(event) => {
              if ((event.target as HTMLElement).closest("a, button, input, label")) return;
              if (window.getSelection()?.toString()) return;
              openPeek(session.session_id);
            }}
            className={cn(
              "cursor-pointer",
              peeked === session.session_id ? "bg-muted" : "hover:bg-muted/50",
            )}
          >
            <Td className={cn("max-w-64 truncate text-xs", tight)} title={session.session_id}>
              <SessionLink
                sessionId={session.session_id}
                href={sessionHref(organization, slug, session.session_id, { days: query.days })}
              >
                {session.session_id}
              </SessionLink>
            </Td>
            {columns.map((column) =>
              column.key === "user" ? (
                <Td
                  key={column.key}
                  className={cn("max-w-48 truncate text-xs", tight)}
                  title={session.user_id || undefined}
                >
                  {session.user_id ? (
                    <Link
                      href={tracesHref(organization, slug, {
                        user: [session.user_id],
                        days: query.days,
                      })}
                      className="underline-offset-4 hover:underline"
                    >
                      {session.user_id}
                    </Link>
                  ) : (
                    <span className="text-muted-foreground">—</span>
                  )}
                </Td>
              ) : (
                <Td
                  key={column.key}
                  className={cn(sumClass(column.key, session), tight)}
                  title={sumTitle(column.key, session)}
                >
                  {sumCell(column.key, session)}
                </Td>
              ),
            )}
          </tr>
        ))}
      </tbody>
    </DataTable>
  );
}
