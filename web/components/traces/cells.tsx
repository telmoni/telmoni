import type { ReactNode } from "react";
import Link from "next/link";

import { LinkPending } from "@/components/link-pending";
import { LocalTime } from "@/components/local-time";
import { TraceStatusBadge } from "@/components/trace-status-badge";
import { Badge } from "@/components/ui/badge";
import type { Trace, TraceQuery } from "@/lib/server/entities/traces";
import { scoreTitle, scoreWords } from "@/lib/scores";
import type { ScoreSummary } from "@/lib/types/traces";
import { formatCost, formatCount, formatDuration, type ColumnSpec, type FilterKey } from "@/lib/traces";
import { TraceColumn } from "@/lib/types/traces";

// One trace's cells, as the trace list draws them (`table.tsx`).

/** A cell's look: a number stands right in figures of one width, an id is
 *  cut short where it runs long. */
export function cellClass(column: ColumnSpec): string {
  if (column.numeric) return "whitespace-nowrap text-right tabular-nums";
  switch (column.key) {
    case TraceColumn.User:
    case TraceColumn.Session:
      return "max-w-48 truncate text-xs";
    case TraceColumn.Started:
    case TraceColumn.Ended:
      return "whitespace-nowrap text-xs text-muted-foreground";
    case TraceColumn.Agent:
      return "whitespace-nowrap font-medium";
    case TraceColumn.ErrorType:
    case TraceColumn.Environment:
    case TraceColumn.Release:
      return "max-w-48 truncate text-xs";
    case TraceColumn.Models:
    case TraceColumn.Tools:
      return "max-w-64 truncate text-xs";
    case TraceColumn.Tags:
    case TraceColumn.Scores:
      return "max-w-64 text-xs";
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
      return `${formatCount(trace.model_calls)} model calls, ${formatCount(trace.tool_calls)} tool calls, ${formatCount(trace.retrievals)} retrievals`;
    case TraceColumn.Tokens:
      return `${formatCount(trace.input_tokens)} in, ${formatCount(trace.output_tokens)} out`;
    case TraceColumn.Models:
      return trace.models.join(", ") || undefined;
    case TraceColumn.Tools:
      return trace.tools.join(", ") || undefined;
    case TraceColumn.Environment:
      return trace.environments.join(", ") || undefined;
    case TraceColumn.Release:
      return trace.releases.join(", ") || undefined;
    case TraceColumn.Tags:
      return trace.tags.join(", ") || undefined;
    case TraceColumn.Scores:
      return trace.scores.map((score) => `${score.name}: ${scoreWords(score)}`).join(", ") || undefined;
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
    case TraceColumn.Retrievals:
      return formatCount(trace.retrievals);
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
    case TraceColumn.Spend:
      return formatCost(trace.spend);
    case TraceColumn.Duration:
      return formatDuration(trace.duration_ms);
    case TraceColumn.TimeToFirstToken:
      return trace.time_to_first_token_ms === null ? (
        <span className="text-muted-foreground">—</span>
      ) : (
        formatDuration(trace.time_to_first_token_ms)
      );
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
    case TraceColumn.Environment:
      return (
        <Values
          values={trace.environments}
          current={query.environment}
          href={narrowedTo ? (v) => narrowedTo("environment", v) : null}
        />
      );
    case TraceColumn.Release:
      return (
        <Values
          values={trace.releases}
          current={query.release}
          href={narrowedTo ? (v) => narrowedTo("release", v) : null}
        />
      );
    case TraceColumn.Tags:
      return (
        <TagChips
          tags={trace.tags}
          current={query.tag}
          href={narrowedTo ? (v) => narrowedTo("tag", v) : null}
        />
      );
    case TraceColumn.Scores:
      return <ScoreChips scores={trace.scores} />;
  }
}

/** A run's or a session's scores, each name and what it says a chip; a dash where it has none. */
export function ScoreChips({ scores }: { scores: readonly ScoreSummary[] }) {
  if (scores.length === 0) return <span className="text-muted-foreground">—</span>;
  return (
    <span className="flex flex-wrap gap-1">
      {scores.map((score) => (
        <Badge key={score.name} variant="outline" className="max-w-full" title={scoreTitle(score)}>
          <span className="truncate">
            {score.name} <span className="tabular-nums text-muted-foreground">{scoreWords(score)}</span>
          </span>
        </Badge>
      ))}
    </span>
  );
}

/** A row's tags, each `key:value` a chip, and a filter where the page offers
 *  one; a dash where it has none. */
export function TagChips({
  tags,
  current,
  href = null,
}: {
  tags: readonly string[];
  current?: readonly string[];
  href?: ((tag: string) => string) | null;
}) {
  if (tags.length === 0) return <span className="text-muted-foreground">—</span>;
  return (
    <span className="flex flex-wrap gap-1">
      {tags.map((tag) =>
        href && !(current?.length === 1 && current[0] === tag) ? (
          <Badge key={tag} variant="secondary" className="max-w-full" asChild>
            <Link href={href(tag)} scroll={false}>
              <span className="truncate">{tag}</span>
              <LinkPending />
            </Link>
          </Badge>
        ) : (
          <Badge key={tag} variant="secondary" className="max-w-full">
            <span className="truncate">{tag}</span>
          </Badge>
        ),
      )}
    </span>
  );
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
