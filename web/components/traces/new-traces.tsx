"use client";

import { useRouter } from "next/navigation";
import { useLayoutEffect, useRef, useState, useTransition } from "react";

import { Button } from "@/components/ui/button";
import { formatCount } from "@/lib/traces";
import { listDrawn, tracesShown, useNewTraces, type NewTraces as Counted } from "@/lib/use-new-traces";

// The traces that landed since this list was drawn, counted above it rather
// than put into it: a list that draws itself again moves its rows under the
// reader's hand, so this one waits to be asked. `RealtimeListener` counts
// them (`lib/use-new-traces.ts`), and draws an empty list itself, since
// nothing on it can move. `drawn` is a token of the rows the server drew
// this time, the table's key: those rows are no longer new, whatever drew
// them — Show, Refresh, a filter — and what was counted and not drawn
// stands, the filters being what they are. Keyed by the project where it is
// rendered, so a project change is a first draw.
export function NewTraces({
  projectId,
  drawn,
  empty,
}: {
  projectId: string;
  drawn: string;
  empty: boolean;
}) {
  const router = useRouter();
  const [showing, start] = useTransition();
  const counted = useNewTraces(projectId);
  // What the reader was shown, kept up while the list is on its way.
  const [shown, setShown] = useState<Counted | null>(null);

  // Before paint, so a count left from an earlier visit never shows: the
  // first draw stands before nothing counted.
  const drawnOnce = useRef(false);
  useLayoutEffect(() => {
    listDrawn(projectId, {
      rows: drawn.split(",").filter(Boolean),
      empty,
      first: !drawnOnce.current,
    });
    drawnOnce.current = true;
  }, [projectId, drawn, empty]);

  const view = showing && shown ? shown : counted;
  if (empty || view.count === 0) return null;
  const noun = view.count === 1 && !view.more ? "trace" : "traces";
  return (
    <div
      role="status"
      className="flex items-center justify-between gap-3 rounded-md bg-muted px-3 py-1.5 text-sm"
    >
      <span>
        {formatCount(view.count)}
        {view.more ? "+" : ""} new {noun}
      </span>
      {/* Not disabled while it shows: a disabled button fades the one word
          saying what is happening and lets focus fall to the page. */}
      <Button
        size="sm"
        variant="outline"
        className="bg-background"
        aria-disabled={showing || undefined}
        aria-busy={showing || undefined}
        data-pending={showing ? "" : undefined}
        onClick={() => {
          if (showing) return;
          setShown(counted);
          tracesShown(projectId);
          start(() => router.refresh());
        }}
      >
        {showing ? "Showing…" : "Show"}
      </Button>
    </div>
  );
}
