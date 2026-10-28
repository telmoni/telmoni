"use client";

import { memo, useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";

import {
  INDENT_MAX,
  KindIcon,
  SpanBar,
  SpanFacts,
  SpanGuides,
  TruncatedNote,
  indentStyle,
} from "@/components/traces/span-parts";
import {
  openingFor,
  shutBranches,
  spanTitle,
  visibleTree,
  type LayoutOptions,
} from "@/lib/layout";
import type { TraceDetail } from "@/lib/server/entities/traces";
import { spanTree, toggled, treeRows, type SpanNode } from "@/lib/trace-tree";
import { barsById, type TimelineBar } from "@/lib/traces";
import { SpanStatus } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

// A trace opened under a session's page, as far as the layout opens it: its
// steps, or its steps and the calls inside them, each named by what it is
// and nested under what made it, as the trace page's tree nests them
// (`trace-view.tsx`), a list to read rather than a tree to walk — the
// chevrons are its only controls — and each span's bar on the trace's own
// line of time where the layout draws one. Metadata alone, as every page is.

export const TraceOutline = memo(function TraceOutline({
  trace,
  options,
  className,
}: {
  trace: TraceDetail;
  options: LayoutOptions;
  className?: string;
}) {
  const nodes = useMemo(
    () => visibleTree(spanTree(trace.spans), options),
    [trace.spans, options],
  );
  const bars = useMemo(() => barsById(trace.spans), [trace.spans]);
  const [shut, setShut] = useState<ReadonlySet<string>>(() =>
    shutBranches(nodes, openingFor(trace.spans.length, options.open)),
  );
  // The run's own row is the trace's, drawn above: its spans start the list.
  const rows = treeRows(nodes, shut).filter((node) => node.depth > 0);
  const toggle = (id: string) => setShut((was) => toggled(was, id));

  if (rows.length === 0) {
    return (
      <p className={cn("px-2 py-1.5 text-xs text-muted-foreground", className)}>
        {trace.spans.length > 1
          ? "No span of this trace is shown under this layout."
          : "No span under the run yet."}
      </p>
    );
  }
  return (
    <div role="list" aria-label="Spans" className={cn("grid", className)}>
      {rows.map((node) => (
        <OutlineRow
          key={node.span.span_id}
          node={node}
          open={!shut.has(node.span.span_id)}
          bar={options.timeline ? (bars.get(node.span.span_id) ?? null) : null}
          onToggle={toggle}
        />
      ))}
      <TruncatedNote trace={trace} className="px-2 py-1.5" />
    </div>
  );
});

/** One span's row: its kind, what it is, how long it ran, a model call's
 *  tokens and a failure's class, under the spans it ran inside. */
function OutlineRow({
  node,
  open,
  bar,
  onToggle,
}: {
  node: SpanNode;
  open: boolean;
  bar: TimelineBar | null;
  onToggle: (id: string) => void;
}) {
  const { span } = node;
  const depth = Math.min(node.depth, INDENT_MAX) - 1;
  const branch = node.children.length > 0;
  return (
    <div
      role="listitem"
      aria-level={node.depth}
      aria-setsize={node.siblings}
      aria-posinset={node.position}
      data-span={span.span_id}
      className={cn("relative flex items-start gap-1.5 rounded-md py-1 pr-2", bar && "pb-4")}
      style={indentStyle(depth)}
    >
      <SpanGuides depth={depth} />
      {branch ? (
        <button
          type="button"
          aria-label={open ? "Shut the branch" : "Open the branch"}
          aria-expanded={open}
          onClick={() => onToggle(span.span_id)}
          className="flex h-5 w-4 shrink-0 cursor-pointer items-center justify-center text-muted-foreground hover:text-foreground"
        >
          <ChevronRight className={cn("size-3.5 transition-transform", open && "rotate-90")} />
        </button>
      ) : (
        <span aria-hidden="true" className="h-5 w-4 shrink-0" />
      )}
      <KindIcon span={span} className="mt-0.5" />
      <span className="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span className="truncate text-sm" title={span.name}>
          {spanTitle(span)}
        </span>
        <SpanFacts span={span} />
      </span>
      {bar && (
        <SpanBar bar={bar} failed={span.status === SpanStatus.Error} className="bottom-1.5" />
      )}
    </div>
  );
}
