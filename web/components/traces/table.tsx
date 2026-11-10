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
import { usePeekedTrace } from "@/components/traces/peek";
import { openPeek } from "@/lib/peek";
import type { Trace, TraceQuery } from "@/lib/server/entities/traces";
import {
  TRACE_COLUMNS,
  currentSort,
  formatCount,
  groupOf,
  nextSort,
  sessionTracesHref,
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

// The page's rows, a trace each: its id, which opens it, with a failed
// trace's class under it — metadata, so a redraw files no read; its message
// is content, the peek's — and the columns the person shows
// (`lib/trace-prefs.ts`), each heading that sorts a link that sorts by it. A
// click on a row opens the trace in the peek beside the list (`peek.tsx`),
// and the row stays marked while it is open. Grouped by session or user
// (`groupOf`), each group is a body of its own under a head that opens the
// group's traces.
export function TraceTable({ organization, slug, query, ui, traces, knownTags }: Props) {
  const prefs = useTracePrefs();
  const peeked = usePeekedTrace();
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
      {groupsOf(traces, group).map((rows, at, groups) => (
        // Keyed by the group, never its first row: a trace landing on top
        // leaves the rows under it, and the keyboard's place among them.
        <tbody key={bodyKey(groups, at, group)}>
          {group && (
            <GroupHead
              group={group}
              value={valueOf(rows[0]!, group)}
              count={rows.length}
              span={columns.length + 1}
              href={(value) =>
                group === "session"
                  ? sessionTracesHref(organization, slug, value, query.days)
                  : tracesHref(organization, slug, { user: [value], days: query.days })
              }
            />
          )}
          {rows.map((trace) => (
            <tr
              key={trace.trace_id}
              aria-current={peeked === trace.trace_id ? "true" : undefined}
              // Anywhere on the row but what keeps its own way: the
              // keyboard's way is the id's link.
              onClick={(event) => {
                if (rowClickTaken(event)) return;
                openPeek(trace.trace_id);
              }}
              className={rowClass(peeked === trace.trace_id)}
            >
              <Td className={cn("whitespace-nowrap", tight)}>
                <Link
                  href={traceHref(organization, slug, trace.trace_id)}
                  data-peek-trace={trace.trace_id}
                  // A plain click opens the trace in the peek; one that asks
                  // for a new tab or window, or a copy of the link, gets the
                  // trace's own page, as the link says.
                  onClick={(event) => {
                    if (!plainClick(event)) return;
                    event.preventDefault();
                    openPeek(trace.trace_id);
                  }}
                  className="text-xs underline-offset-4 hover:underline"
                >
                  {shortId(trace.trace_id)}
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

/** A trace's value of `group`; empty where it named none. */
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

/** A body's key: its group's value, numbered where a value heads two groups. */
function bodyKey(groups: Trace[][], at: number, group: Group | undefined): string {
  if (!group) return "all";
  const value = valueOf(groups[at]![0]!, group);
  const before = groups.slice(0, at).filter((rows) => valueOf(rows[0]!, group) === value);
  return `${before.length}:${value}`;
}

/** A group's head: its session or user, which opens the group's traces, and how
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
            title={group === "session" ? `The traces of session ${value}, in order` : `Every trace of user ${value}`}
            className="font-medium underline-offset-4 hover:underline"
          >
            {group === "user" ? shortId(value) : value}
          </Link>
        ) : (
          <span className="text-muted-foreground">none</span>
        )}
        <span className="ml-gutter text-muted-foreground tabular-nums">
          {formatCount(count)} {count === 1 ? "trace" : "traces"} on this page
        </span>
      </th>
    </tr>
  );
}
