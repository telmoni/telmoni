"use client";

import { useState, type ReactNode } from "react";
import { DownloadIcon } from "lucide-react";
import Link from "next/link";

import { DataTable, THead, Td, Th } from "@/components/data-table";
import { LinkPending } from "@/components/link-pending";
import { LocalTime } from "@/components/local-time";
import { SortHeading } from "@/components/sort-heading";
import { TraceStatusBadge } from "@/components/trace-status-badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { Trace, TraceQuery } from "@/lib/server/entities/traces";
import {
  TRACE_COLUMNS,
  currentSort,
  downloadHref,
  formatCount,
  formatDuration,
  nextSort,
  shortId,
  traceHref,
  tracesHref,
  type ColumnSpec,
  type FilterKey,
  type TraceUi,
} from "@/lib/traces";
import { TraceColumn } from "@/lib/types/enums";
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
// link that sorts by it. Rows are chosen, as Langfuse's are, to download
// those alone; the choice is the page's, so a page of new rows starts with
// none chosen (the page keys this on its rows).
export function TraceTable({ organization, slug, query, ui, traces }: Props) {
  const prefs = useTracePrefs();
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
            <tr key={trace.run_id} data-state={chosen.has(trace.run_id) ? "selected" : undefined}>
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
                  className="font-mono text-xs underline-offset-4 hover:underline"
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

/** A cell's look: a number stands right in figures of one width, an id in
 *  the mono face, cut short where it runs long. */
function cellClass(column: ColumnSpec): string {
  if (column.numeric) return "whitespace-nowrap text-right tabular-nums";
  switch (column.key) {
    case TraceColumn.User:
    case TraceColumn.Session:
      return "max-w-48 truncate font-mono text-xs";
    case TraceColumn.Started:
    case TraceColumn.Ended:
      return "whitespace-nowrap text-xs text-muted-foreground";
    case TraceColumn.Agent:
      return "whitespace-nowrap font-medium";
    case TraceColumn.Models:
    case TraceColumn.Tools:
      return "max-w-64 truncate text-xs";
    default:
      return "whitespace-nowrap";
  }
}

/** What a cell says on hover, where its face says less. */
function titleOf(column: ColumnSpec, trace: Trace): string | undefined {
  switch (column.key) {
    case TraceColumn.User:
      return trace.user_id || undefined;
    case TraceColumn.Session:
      return trace.session_id || undefined;
    case TraceColumn.Steps:
      return `${formatCount(trace.model_calls)} model calls, ${formatCount(trace.tool_calls)} tool calls`;
    case TraceColumn.Tokens:
      return `${formatCount(trace.input_tokens)} in, ${formatCount(trace.output_tokens)} out`;
    case TraceColumn.Models:
      return trace.models.join(", ") || undefined;
    case TraceColumn.Tools:
      return trace.tools.join(", ") || undefined;
    default:
      return undefined;
  }
}

/** A cell, as its column reads the run. */
function cellOf(
  column: ColumnSpec,
  trace: Trace,
  query: TraceQuery,
  narrowedTo: (filter: FilterKey, value: string) => string,
): ReactNode {
  switch (column.key) {
    case TraceColumn.Started:
      return <LocalTime iso={trace.started_at} />;
    case TraceColumn.Ended:
      return trace.ended ? <LocalTime iso={trace.ended_at} /> : "Running";
    case TraceColumn.Agent:
      return <FilterLink value={trace.agent} current={query.agent} href={narrowedTo("agent", trace.agent)} />;
    case TraceColumn.User:
      return <FilterLink value={trace.user_id} current={query.user} href={narrowedTo("user", trace.user_id)} />;
    case TraceColumn.Session:
      return (
        <FilterLink
          value={trace.session_id}
          current={query.session}
          href={narrowedTo("session", trace.session_id)}
        />
      );
    case TraceColumn.Status:
      return <TraceStatusBadge trace={trace} />;
    case TraceColumn.Steps:
      return formatCount(trace.steps);
    case TraceColumn.ModelCalls:
      return formatCount(trace.model_calls);
    case TraceColumn.ToolCalls:
      return formatCount(trace.tool_calls);
    case TraceColumn.Errors:
      return formatCount(trace.errors);
    case TraceColumn.Spans:
      return formatCount(trace.spans);
    case TraceColumn.InputTokens:
      return formatCount(trace.input_tokens);
    case TraceColumn.OutputTokens:
      return formatCount(trace.output_tokens);
    case TraceColumn.CacheReadTokens:
      return formatCount(trace.cache_read_input_tokens);
    case TraceColumn.CacheWriteTokens:
      return formatCount(trace.cache_write_input_tokens);
    case TraceColumn.Tokens:
      return formatCount(trace.input_tokens + trace.output_tokens);
    case TraceColumn.Duration:
      return formatDuration(trace.duration_ms);
    case TraceColumn.Models:
      return <Values values={trace.models} current={query.model} href={(v) => narrowedTo("model", v)} />;
    case TraceColumn.Tools:
      return <Values values={trace.tools} current={query.tool} href={(v) => narrowedTo("tool", v)} />;
  }
}

/** A cell whose value is a filter: a link that narrows the list to it, plain
 *  once the list is narrowed to it alone, a dash when the run sent none. */
function FilterLink({
  value,
  current,
  href,
}: {
  value: string;
  current: readonly string[] | undefined;
  href: string;
}) {
  if (!value) return <span className="text-muted-foreground">—</span>;
  if (current?.length === 1 && current[0] === value) return <span>{value}</span>;
  return (
    <Link href={href} scroll={false} className="underline-offset-4 hover:underline">
      {value}
      <LinkPending />
    </Link>
  );
}

/** A cell of several values — a run's models, its tools — each a filter. */
function Values({
  values,
  current,
  href,
}: {
  values: readonly string[];
  current: readonly string[] | undefined;
  href: (value: string) => string;
}) {
  if (values.length === 0) return <span className="text-muted-foreground">—</span>;
  return values.map((value, i) => (
    <span key={value}>
      {i > 0 && ", "}
      <FilterLink value={value} current={current} href={href(value)} />
    </span>
  ));
}
