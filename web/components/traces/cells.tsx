"use client";

import type { ReactNode } from "react";
import Link from "next/link";

import { LinkPending } from "@/components/link-pending";
import { LocalTime } from "@/components/local-time";
import { TraceStatusBadge } from "@/components/trace-status-badge";
import type { Trace, TraceQuery } from "@/lib/server/entities/traces";
import { formatCount, formatDuration, type ColumnSpec, type FilterKey } from "@/lib/traces";
import { TraceColumn } from "@/lib/types/enums";

// One trace's cells, as the trace list draws them (`table.tsx`) and a
// session's page draws its traces under a layout's columns
// (`session-runs.tsx`): the same column reads the same on both.

/** A cell's look: a number stands right in figures of one width, an id in
 *  the mono face, cut short where it runs long. */
export function cellClass(column: ColumnSpec): string {
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
    case TraceColumn.ErrorType:
      return "max-w-48 truncate text-xs";
    case TraceColumn.Models:
    case TraceColumn.Tools:
      return "max-w-64 truncate text-xs";
    default:
      return "whitespace-nowrap";
  }
}

/** What a cell says on hover, where its face says less. */
export function titleOf(column: ColumnSpec, trace: Trace): string | undefined {
  switch (column.key) {
    case TraceColumn.User:
      return trace.user_id || undefined;
    case TraceColumn.Session:
      return trace.session_id || undefined;
    case TraceColumn.ErrorType:
      return trace.error_type || undefined;
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

/** A cell, as its column reads the run. A value that is a filter is a link
 *  to `narrowedTo` where the page offers one, and plain text otherwise. */
export function cellOf(
  column: ColumnSpec,
  trace: Trace,
  query: TraceQuery,
  narrowedTo: ((filter: FilterKey, value: string) => string) | null,
): ReactNode {
  const link = (filter: FilterKey, value: string) =>
    narrowedTo ? (
      <FilterLink value={value} current={query[filter]} href={narrowedTo(filter, value)} />
    ) : (
      <Plain value={value} />
    );
  switch (column.key) {
    case TraceColumn.Started:
      return <LocalTime iso={trace.started_at} />;
    case TraceColumn.Ended:
      return trace.ended ? <LocalTime iso={trace.ended_at} /> : "Running";
    case TraceColumn.Agent:
      return link("agent", trace.agent);
    case TraceColumn.User:
      return link("user", trace.user_id);
    case TraceColumn.Session:
      return link("session", trace.session_id);
    case TraceColumn.Status:
      return <TraceStatusBadge trace={trace} />;
    case TraceColumn.ErrorType:
      return trace.error_type ? (
        <span className="text-destructive">{trace.error_type}</span>
      ) : (
        <span className="text-muted-foreground">—</span>
      );
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
      return (
        <Values
          values={trace.models}
          current={query.model}
          href={narrowedTo ? (v) => narrowedTo("model", v) : null}
        />
      );
    case TraceColumn.Tools:
      return (
        <Values
          values={trace.tools}
          current={query.tool}
          href={narrowedTo ? (v) => narrowedTo("tool", v) : null}
        />
      );
  }
}

/** A value, or a dash where the run sent none. */
function Plain({ value }: { value: string }) {
  if (!value) return <span className="text-muted-foreground">—</span>;
  return <span>{value}</span>;
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

/** A cell of several values — a run's models, its tools — each a filter
 *  where the page offers one. */
function Values({
  values,
  current,
  href,
}: {
  values: readonly string[];
  current: readonly string[] | undefined;
  href: ((value: string) => string) | null;
}) {
  if (values.length === 0) return <span className="text-muted-foreground">—</span>;
  return values.map((value, i) => (
    <span key={value}>
      {i > 0 && ", "}
      {href ? <FilterLink value={value} current={current} href={href(value)} /> : value}
    </span>
  ));
}
