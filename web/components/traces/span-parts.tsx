import { Footprints, ListTree, MoveHorizontal, Sparkles, Wrench, type LucideIcon } from "lucide-react";
import type { CSSProperties, ReactNode } from "react";

import { spanTitle } from "@/lib/layout";
import type { PinnedSpan } from "@/lib/server/entities/sessions";
import type { TraceDetail, TraceSpan } from "@/lib/server/entities/traces";
import {
  STATUS_LABEL,
  formatCount,
  formatDuration,
  spanMillis,
  type TimelineBar,
} from "@/lib/traces";
import { SpanKind, SpanStatus } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

// What a span's row is drawn from, on the trace page and in a session's
// outline alike: its kind's glyph, how far in it stands and the guides to
// there, its bar on the trace's line of time, and what it says beside its
// name. Metadata alone, as every page is.

/** A span's kind as a glyph: the run as the Traces row draws it, a model
 *  call as the sparkle every AI surface uses, a tool as its wrench. */
const KIND_ICON: Record<SpanKind, LucideIcon> = {
  [SpanKind.Run]: ListTree,
  [SpanKind.Step]: Footprints,
  [SpanKind.ModelCall]: Sparkles,
  [SpanKind.ToolCall]: Wrench,
  [SpanKind.Span]: MoveHorizontal,
};

/** A span's kind, in the error's colour where it failed. */
export function KindIcon({
  span,
  className,
}: {
  span: Pick<TraceSpan, "kind" | "status">;
  className?: string;
}) {
  const Icon = KIND_ICON[span.kind];
  return (
    <Icon
      aria-hidden="true"
      className={cn(
        "size-4 shrink-0",
        span.status === SpanStatus.Error ? "text-destructive" : "text-muted-foreground",
        className,
      )}
    />
  );
}

/** How many levels a row steps in before it holds its place: a deeper span
 *  reads by its guides rather than running off the pane. */
export const INDENT_MAX = 8;

/** How far each level steps in, in rem; the guides stand on the same measure. */
const INDENT_REM = 1.25;

/** The row stepped in to its depth. */
export function indentStyle(depth: number): CSSProperties {
  return { paddingLeft: `${depth * INDENT_REM}rem` };
}

/** A guide standing for each span the row ran inside. */
export function SpanGuides({ depth }: { depth: number }) {
  return Array.from({ length: depth }, (_, level) => (
    <span
      key={level}
      aria-hidden="true"
      className="absolute inset-y-0 border-l border-border"
      style={{ left: `${level * INDENT_REM + 0.5}rem` }}
    />
  ));
}

/** The row's bar on the trace's one line of time: from where the run's own
 *  name starts to the pane's edge, whatever the row's depth, so bars read
 *  against each other as a timeline's must. */
export function SpanBar({
  bar,
  failed,
  className,
}: {
  bar: Pick<TimelineBar, "offset" | "width">;
  failed: boolean;
  className?: string;
}) {
  return (
    <span
      aria-hidden="true"
      className={cn("absolute right-2 left-11 h-1.5 rounded-full bg-border", className)}
    >
      <span
        className={cn("absolute inset-y-0 rounded-full", failed ? "bg-destructive" : "bg-foreground/60")}
        style={{ left: `${bar.offset}%`, width: `${bar.width}%` }}
      />
    </span>
  );
}

/** What a span's row says beside its name: how long it ran, a model call's
 *  tokens, and a failure's class. */
export function SpanFacts({
  span,
  tokens = true,
}: {
  span: Pick<
    TraceSpan,
    "kind" | "status" | "error_type" | "started_at" | "ended_at" | "input_tokens" | "output_tokens"
  >;
  tokens?: boolean;
}) {
  return (
    <>
      <Fact>{formatDuration(spanMillis(span))}</Fact>
      {tokens && span.kind === SpanKind.ModelCall && (
        <Fact>{formatCount(span.input_tokens + span.output_tokens)} tokens</Fact>
      )}
      {span.status === SpanStatus.Error && (
        <span className="text-xs text-destructive">{span.error_type || STATUS_LABEL[span.status]}</span>
      )}
    </>
  );
}

function Fact({ children }: { children: ReactNode }) {
  return <span className="text-xs tabular-nums text-muted-foreground">{children}</span>;
}

/** A span the layout pinned, as its chip reads: its kind, what it is, how
 *  long it ran and a failure's class. */
export function PinnedChip({ span }: { span: PinnedSpan }) {
  return (
    <>
      <KindIcon span={span} className="size-3.5" />
      <span className="truncate">{spanTitle(span)}</span>
      <SpanFacts span={span} tokens={false} />
    </>
  );
}

/** When the server sent the first spans alone. */
export function TruncatedNote({
  trace,
  className,
}: {
  trace: Pick<TraceDetail, "truncated" | "spans">;
  className?: string;
}) {
  if (!trace.truncated) return null;
  return (
    <p className={cn("text-xs text-muted-foreground", className)}>
      This run holds more spans than the server sends at once; the first{" "}
      {formatCount(trace.spans.length)} are here.
    </p>
  );
}

/** `set` with `id` taken out where it is in, else put in. */
export function toggled(set: ReadonlySet<string>, id: string): Set<string> {
  const next = new Set(set);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}
