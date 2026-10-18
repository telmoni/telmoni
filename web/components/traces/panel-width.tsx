"use client";

import { useRef, useState, type PointerEvent as ReactPointerEvent, type RefObject } from "react";

import { draggedWidth } from "@/lib/peek";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";
import { cn } from "@/lib/utils";

// How wide a panel a list opens stands, full size or dragged: one choice for
// every such panel — a trace's over the trace list, a session's over the
// sessions list — kept with the list's other choices (`lib/trace-prefs.ts`).

/** The edge's handlers and look, for `PanelEdge`; `null` at full size,
 *  where there is no edge to drag. */
export type PanelEdgeProps = {
  onPointerDown: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onPointerMove: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onPointerUp: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onPointerCancel: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onLostPointerCapture: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onDoubleClick: () => void;
  className: string;
};

/** The panel's width and whether it fills the content area, as the person
 *  set them; its left edge dragged, as the agent's window is sized by its
 *  own (`agent-window.tsx`): followed as the pointer moves, and kept once it
 *  lets go — one write a drag. */
export function usePanelWidth(panel: RefObject<HTMLDivElement | null>): {
  full: boolean;
  width: number | null;
  toggleFull: () => void;
  edge: PanelEdgeProps | null;
} {
  const { peekFull, peekWidth } = useTracePrefs();
  const [dragged, setDragged] = useState<number | null>(null);
  const drag = useRef<{ pointerId: number; x: number; start: number; width: number | null } | null>(
    null,
  );
  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || !e.isPrimary || !panel.current) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    const start = panel.current.getBoundingClientRect().width;
    drag.current = { pointerId: e.pointerId, x: e.clientX, start, width: null };
  };
  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    const area = panel.current?.parentElement?.getBoundingClientRect().width;
    if (!d || d.pointerId !== e.pointerId || !area) return;
    d.width = draggedWidth(d.start, d.x, e.clientX, area);
    setDragged(d.width);
  };
  const end = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || d.pointerId !== e.pointerId) return;
    drag.current = null;
    if (d.width !== null) setTracePrefs({ peekWidth: d.width });
    setDragged(null);
  };
  return {
    full: peekFull,
    width: dragged ?? peekWidth,
    toggleFull: () => setTracePrefs({ peekFull: !peekFull }),
    edge: peekFull
      ? null
      : {
          onPointerDown,
          onPointerMove,
          onPointerUp: end,
          onPointerCancel: end,
          onLostPointerCapture: end,
          // Back at its own width.
          onDoubleClick: () => setTracePrefs({ peekWidth: null }),
          className: cn(
            "absolute inset-y-0 left-0 z-10 hidden w-1.5 cursor-ew-resize touch-none md:block",
            "hover:bg-foreground/10",
            dragged !== null && "bg-foreground/10",
          ),
        },
  };
}

/** The edge to drag, on the panel's own left edge: a pointer's alone, Full
 *  size being the keyboard's way to more room. */
export function PanelEdge({ edge }: { edge: PanelEdgeProps | null }) {
  if (!edge) return null;
  return <div aria-hidden="true" data-testid="peek-resize" {...edge} />;
}
