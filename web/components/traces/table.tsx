"use client";

import Link from "next/link";

import { DataTable, THead, Td, Th } from "@/components/data-table";
import { cellClass, cellOf, titleOf } from "@/components/traces/cells";
import {
  ColumnHeadings,
  compactPadding,
  plainClick,
  rowClass,
  rowClickTaken,
} from "@/components/traces/list-table";
import { usePeekedRun } from "@/components/traces/peek";
import { openPeek } from "@/lib/peek";
import type { Trace, TraceQuery } from "@/lib/server/entities/traces";
import {
  TRACE_COLUMNS,
  currentSort,
  formatCount,
  groupOf,
  nextSort,
  sessionRunsHref,
  shortId,
  tagKeys,
  traceHref,
  tracesHref,
  type FilterKey,
  type Group,
  type TraceUi,
} from "@/lib/traces";
import { SpanStatus, type TraceSort } from "@/lib/types/enums";
import type { TagSort } from "@/lib/types/traces";
import { useTracePrefs } from "@/lib/use-trace-prefs";
import { cn } from "@/lib/utils";

type Props = {
  organization: string;
  slug: string;
  query: TraceQuery;
  ui: TraceUi;
  traces: Trace[];
  /** The window's tags, each `key:value`, whose keys the Tags heading sorts by. */
  knownTags: readonly string[];
};

// The page's rows, a run each: its id, which opens its trace, with a failed
// run's class under it — metadata, so a redraw files no read; its message is
// content, the peek's — and the columns the person shows
// (`lib/trace-prefs.ts`), each heading that sorts a link that sorts by it. A
// click on a row opens its trace in the peek beside the list (`peek.tsx`),
// and the row stays marked while it is open. Grouped by session or user
// (`groupOf`), each group is a body of its own under a head that opens the
// group's runs.
export function TraceTable({ organization, slug, query, ui, traces, knownTags }: Props) {
  const prefs = useTracePrefs();
  const peeked = usePeekedRun();
  const columns = TRACE_COLUMNS.filter((c) => prefs.columns.includes(c.key));
  const { sort, dir } = currentSort(query);
  const tight = compactPadding(prefs.density);
  const narrowedTo = (filter: FilterKey, value: string) =>
    tracesHref(organization, slug, query, { [filter]: [value], page: undefined }, ui);
  const sortedBy = (by: TraceSort | TagSort) =>
    tracesHref(organization, slug, query, nextSort(query, by), ui);
  const group = groupOf(query);

  return (
    <DataTable>
      <THead>
        <Th className="whitespace-nowrap">Trace</Th>
        <ColumnHeadings
          columns={columns}
          sort={sort}
          dir={dir}
          sortHref={sortedBy}
          tags={{ keys: tagKeys(knownTags), href: (key) => sortedBy(`tag:${key}`) }}
        />
      </THead>
      {groupsOf(traces, group).map((rows) => (
        <tbody key={rows[0]!.run_id}>
          {group && (
            <GroupHead
              group={group}
              value={valueOf(rows[0]!, group)}
              count={rows.length}
              span={columns.length + 1}
              href={(value) =>
                group === "session"
                  ? sessionRunsHref(organization, slug, value, query.days)
                  : tracesHref(organization, slug, { user: [value], days: query.days })
              }
            />
          )}
          {rows.map((trace) => (
            <tr
              key={trace.run_id}
              aria-current={peeked === trace.run_id ? "true" : undefined}
              // Anywhere on the row but what keeps its own way: the
              // keyboard's way is the id's link.
              onClick={(event) => {
                if (rowClickTaken(event)) return;
                openPeek(trace.run_id);
              }}
              className={rowClass(peeked === trace.run_id)}
            >
              <Td className={cn("whitespace-nowrap", tight)}>
                <Link
                  href={traceHref(organization, slug, trace.run_id)}
                  data-peek-run={trace.run_id}
                  // A plain click opens the trace in the peek; one that asks
                  // for a new tab or window, or a copy of the link, gets the
                  // trace's own page, as the link says.
                  onClick={(event) => {
                    if (!plainClick(event)) return;
                    event.preventDefault();
                    openPeek(trace.run_id);
                  }}
                  className="text-xs underline-offset-4 hover:underline"
                >
                  {shortId(trace.run_id)}
                </Link>
                {trace.ended && trace.status === SpanStatus.Error && trace.error_type && (
                  <span className="block max-w-48 truncate text-xs text-destructive" title={trace.error_type}>
                    {trace.error_type}
                  </span>
                )}
              </Td>
              {columns.map((column) => (
                <Td
                  key={column.key}
                  className={cn(cellClass(column), tight)}
                  title={titleOf(column, trace)}
                >
                  {cellOf(column, trace, query, narrowedTo)}
                </Td>
              ))}
            </tr>
          ))}
        </tbody>
      ))}
    </DataTable>
  );
}

/** A run's value of `group`; empty where it named none. */
function valueOf(trace: Trace, group: Group): string {
  return group === "session" ? trace.session_id : trace.user_id;
}

/** The page's rows as its groups run, one after another; all of them as one
 *  where the list is not grouped. */
function groupsOf(traces: readonly Trace[], group: Group | undefined): Trace[][] {
  const groups: Trace[][] = [];
  for (const trace of traces) {
    const last = groups.at(-1);
    if (last && (!group || valueOf(last[0]!, group) === valueOf(trace, group))) last.push(trace);
    else groups.push([trace]);
  }
  return groups;
}

/** A group's head: its session or user, which opens the group's runs, and how
 *  many of them this page holds. */
function GroupHead({
  group,
  value,
  count,
  span,
  href,
}: {
  group: Group;
  value: string;
  count: number;
  span: number;
  href: (value: string) => string;
}) {
  const noun = group === "session" ? "Session" : "User";
  return (
    <tr className="bg-muted/40">
      <th scope="rowgroup" colSpan={span} className="px-gutter py-1.5 text-left text-xs font-normal">
        <span className="text-muted-foreground">{noun}:</span>{" "}
        {value ? (
          <Link
            href={href(value)}
            title={group === "session" ? `The runs of session ${value}, in order` : `Every run of user ${value}`}
            className="font-medium underline-offset-4 hover:underline"
          >
            {group === "user" ? shortId(value) : value}
          </Link>
        ) : (
          <span className="text-muted-foreground">none</span>
        )}
        <span className="ml-gutter text-muted-foreground tabular-nums">
          {formatCount(count)} {count === 1 ? "run" : "runs"} on this page
        </span>
      </th>
    </tr>
  );
}
