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
import { sessionHref } from "@/lib/sessions";
import {
  TRACE_COLUMNS,
  currentSort,
  nextSort,
  shortId,
  tagKeys,
  traceHref,
  tracesHref,
  type FilterKey,
} from "@/lib/traces";
import { SpanStatus, type TraceSort } from "@/lib/types/enums";
import type { TagSort } from "@/lib/types/traces";
import { useTracePrefs } from "@/lib/use-trace-prefs";
import { userHref } from "@/lib/users";
import { cn } from "@/lib/utils";

type Props = {
  organization: string;
  slug: string;
  query: TraceQuery;
  traces: Trace[];
  /** The window's tags, each `key:value`, whose keys the Tags heading sorts by. */
  knownTags: readonly string[];
};

// The page's rows, a trace each: its id, which opens it, with a failed
// trace's class under it — metadata, so a redraw files no read; its message
// is content, the peek's — and the columns the person shows
// (`lib/trace-prefs.ts`), each heading that sorts a link that sorts by it. A
// click on a row opens the trace in the peek beside the list (`peek.tsx`),
// and the row stays marked while it is open; its user and its session each
// open their own page, over the window the rows were read in.
export function TraceTable({ organization, slug, query, traces, knownTags }: Props) {
  const prefs = useTracePrefs();
  const peeked = usePeekedTrace();
  const columns = TRACE_COLUMNS.filter((c) => prefs.columns.includes(c.key));
  const { sort, dir } = currentSort(query);
  const tight = compactPadding(prefs.density);
  const narrowedTo = (filter: FilterKey, value: string) =>
    tracesHref(organization, slug, query, { [filter]: [value], page: undefined });
  const pages = {
    user: (id: string) => userHref(organization, slug, id, query.days),
    session: (id: string) => sessionHref(organization, slug, id, query.days),
  };
  const sortedBy = (by: TraceSort | TagSort) => tracesHref(organization, slug, query, nextSort(query, by));

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
      <tbody>
        {traces.map((trace) => (
          <tr
            key={trace.trace_id}
            aria-current={peeked === trace.trace_id ? "true" : undefined}
            // Anywhere on the row but what keeps its own way: the keyboard's
            // way is the id's link.
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
              <Td key={column.key} className={cn(cellClass(column), tight)} title={titleOf(column, trace)}>
                {cellOf(column, trace, query, narrowedTo, pages)}
              </Td>
            ))}
          </tr>
        ))}
      </tbody>
    </DataTable>
  );
}
