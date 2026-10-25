"use client";

import { TraceBody } from "@/components/traces/trace-view";
import { useLayout, type LayoutPage } from "@/components/traces/use-layout";
import { layoutQueryOf } from "@/lib/layout";
import type { TraceDetail } from "@/lib/server/data";

// A trace's page under the layout it stands in: the tree and the span
// chosen, opened as far as the layout says and started afresh when the
// layout changes (the body is keyed on it).
export function TraceLayoutBody(
  props: LayoutPage & { trace: TraceDetail; organization: string; slug: string },
) {
  const { options } = useLayout(props);
  const key = JSON.stringify({ ...layoutQueryOf(options), columns: options.columns });
  return (
    <TraceBody
      key={key}
      trace={props.trace}
      organization={props.organization}
      slug={props.slug}
      layout={options}
      className="min-h-0 flex-1"
    />
  );
}
