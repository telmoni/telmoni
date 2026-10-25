"use client";

import { useState } from "react";
import { DownloadIcon } from "lucide-react";
import Link from "next/link";

import { DataTable, THead, Td, Th } from "@/components/data-table";
import { SortHeading } from "@/components/sort-heading";
import { cellClass, cellOf, titleOf } from "@/components/traces/cells";
import { usePeekedRun } from "@/components/traces/peek";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { openPeek } from "@/lib/peek";
import type { Trace, TraceQuery } from "@/lib/server/entities/traces";
import {
  TRACE_COLUMNS,
  currentSort,
  downloadHref,
  formatCount,
  nextSort,
  shortId,
  traceHref,
  tracesHref,
  type FilterKey,
  type TraceUi,
} from "@/lib/traces";
import { useTracePrefs } from "@/lib/use-trace-prefs";
import { cn } from "@/lib/utils";

type Props = {
  organization: string;
  slug: string;
  query: TraceQuery;
  ui: TraceUi;
  traces: Trace[];
};

// The page's rows, a run each: its id, which opens its trace, and the
// columns the person shows (`lib/trace-prefs.ts`), each heading that sorts a
// link that sorts by it. A click on a row opens its trace in the peek beside
// the list (`peek.tsx`), and the row stays marked while it is open. Rows are
// chosen to download those alone; the choice is the page's, so a page of new
// rows starts with none chosen (the page keys this on its rows).
export function TraceTable({ organization, slug, query, ui, traces }: Props) {
  const prefs = useTracePrefs();
  const peeked = usePeekedRun();
  const [chosen, setChosen] = useState<ReadonlySet<string>>(() => new Set());
  const columns = TRACE_COLUMNS.filter((c) => prefs.columns.includes(c.key));
  const { sort, dir } = currentSort(query);
  const tight = prefs.density === "compact" ? "py-1.5" : undefined;
  const every = traces.length > 0 && traces.every((t) => chosen.has(t.run_id));
  const some = !every && traces.some((t) => chosen.has(t.run_id));
  const narrowedTo = (filter: FilterKey, value: string) =>
    tracesHref(organization, slug, query, { [filter]: [value], page: undefined }, ui);

  const choose = (runId: string, on: boolean) =>
    setChosen((was) => {
      const next = new Set(was);
      if (on) next.add(runId);
      else next.delete(runId);
      return next;
    });

  return (
    <div className="grid gap-2">
      {chosen.size > 0 && (
        <div className="flex flex-wrap items-center gap-2 rounded-md bg-muted px-3 py-1.5 text-sm" role="status">
          <span>
            {formatCount(chosen.size)} {chosen.size === 1 ? "trace" : "traces"} chosen
          </span>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="outline" size="sm" className="ml-auto bg-background">
                <DownloadIcon />
                Download
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem asChild>
                <a href={downloadHref(organization, slug, query, "csv", [...chosen])} download>
                  CSV, for a spreadsheet
                </a>
              </DropdownMenuItem>
              <DropdownMenuItem asChild>
                <a href={downloadHref(organization, slug, query, "json", [...chosen])} download>
                  JSON
                </a>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <Button variant="ghost" size="sm" onClick={() => setChosen(new Set())}>
            Clear
          </Button>
        </div>
      )}
      <DataTable>
        <THead>
          <Th className="w-0 pr-0">
            <Checkbox
              checked={every ? true : some ? "indeterminate" : false}
              onCheckedChange={(on) =>
                setChosen(on === true ? new Set(traces.map((t) => t.run_id)) : new Set())
              }
              aria-label="Choose every trace on this page"
            />
          </Th>
          <Th className="whitespace-nowrap">Trace</Th>
          {columns.map((column) =>
            column.sort ? (
              <SortHeading
                key={column.key}
                label={column.label}
                column={column.sort}
                sort={sort}
                dir={dir}
                href={tracesHref(organization, slug, query, nextSort(query, column.sort), ui)}
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
          {traces.map((trace) => (
            <tr
              key={trace.run_id}
              data-state={chosen.has(trace.run_id) ? "selected" : undefined}
              aria-current={peeked === trace.run_id ? "true" : undefined}
              // Anywhere on the row but a control or a link of its own, which
              // keep their own way, and but a drag that chose some text to
              // copy: the keyboard's way is the id's link.
              onClick={(event) => {
                if ((event.target as HTMLElement).closest("a, button, input, label")) return;
                if (window.getSelection()?.toString()) return;
                openPeek(trace.run_id);
              }}
              className={cn(
                "cursor-pointer",
                peeked === trace.run_id ? "bg-muted" : "hover:bg-muted/50",
              )}
            >
              <Td className={cn("pr-0", tight)}>
                <Checkbox
                  checked={chosen.has(trace.run_id)}
                  onCheckedChange={(on) => choose(trace.run_id, on === true)}
                  aria-label={`Choose trace ${shortId(trace.run_id)}`}
                />
              </Td>
              <Td className={cn("whitespace-nowrap", tight)}>
                <Link
                  href={traceHref(organization, slug, trace.run_id)}
                  data-peek-run={trace.run_id}
                  // A plain click opens the trace in the peek; one that asks
                  // for a new tab or window, or a copy of the link, gets the
                  // trace's own page, as the link says.
                  onClick={(event) => {
                    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
                      return;
                    }
                    event.preventDefault();
                    openPeek(trace.run_id);
                  }}
                  className="text-xs underline-offset-4 hover:underline"
                >
                  {shortId(trace.run_id)}
                </Link>
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
      </DataTable>
    </div>
  );
}
