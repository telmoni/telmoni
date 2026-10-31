"use client";

import { RunTurn } from "@/components/traces/span-content";
import { TraceBody } from "@/components/traces/trace-view";
import { useLayout } from "@/components/traces/use-layout";
import { layoutQueryOf } from "@/lib/layout";
import { fieldsOf } from "@/lib/messages";
import type { TraceDetail } from "@/lib/server/data";
import { summarizeTrace } from "@/lib/trace-tree";

// A trace's page under the layout it stands in: the run's turn of a
// conversation where the layout shows messages, then the tree and the span
// chosen, opened as far as the layout says and started afresh when the
// layout changes (the body is keyed on it).
export function TraceLayoutBody(props: { trace: TraceDetail; organization: string; slug: string }) {
  const { options } = useLayout();
  const key = JSON.stringify({ ...layoutQueryOf(options), columns: options.columns });
  const { root } = summarizeTrace(props.trace.spans);
  return (
    <>
      {options.messages && (
        <div className="max-h-56 shrink-0 overflow-y-auto border-b border-border p-gutter">
          <RunTurn fields={fieldsOf(props.trace.content, root.span_id)} />
        </div>
      )}
      <TraceBody
        key={key}
        trace={props.trace}
        organization={props.organization}
        slug={props.slug}
        layout={options}
        className="min-h-0 flex-1"
      />
    </>
  );
}
