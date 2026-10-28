"use client";

import {
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
} from "react";

import { usePointerGesture } from "@/components/use-pointer-gesture";
import {
  FILTERS_WIDTH_DEFAULT,
  FILTERS_WIDTH_MAX,
  FILTERS_WIDTH_MIN,
  FILTERS_WIDTH_STEP,
  draggedPanelWidth,
  filtersBounds,
  type DragBounds,
} from "@/lib/panel-width";
import { PEEK_WIDTH_MIN } from "@/lib/peek";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";
import { cn } from "@/lib/utils";

// How wide a panel beside a list stands, dragged by its edge: one mechanism
// for every such panel — a trace's or a session's over its list, the filters
// beside the trace list — each width kept with the list's other choices
// (`lib/trace-prefs.ts`), followed as the pointer moves and written once it
// lets go, as the agent's window is sized by its own (`agent-window.tsx`).

type Pointer = ReactPointerEvent<HTMLDivElement>;

/** The pointer handlers an edge takes. */
type EdgeHandlers = {
  onPointerDown: (e: Pointer) => void;
  onPointerMove: (e: Pointer) => void;
  onPointerUp: (e: Pointer) => void;
  onPointerCancel: (e: Pointer) => void;
  onLostPointerCapture: (e: Pointer) => void;
  onDoubleClick: () => void;
};

/** The width a drag has the panel at, `null` at rest, and its edge's
 *  handlers: the bounds read off the area the panel stands in, its parent,
 *  as a drag starts; `keep` called once with the width a drag ends at, or
 *  with `null` on a double click, which gives the panel its own width back. */
/** What a drag of the edge holds: where it began and the width then, the
 *  bounds, and the width it has the panel at. */
type Drag = { x: number; start: number; bounds: DragBounds; width: number | null };

function useDraggedEdge(
  panel: RefObject<HTMLElement | null>,
  bounds: (area: number) => DragBounds,
  keep: (width: number | null) => void,
): { dragged: number | null; handlers: EdgeHandlers } {
  const { held, press, handlers } = usePointerGesture<Drag, HTMLDivElement>({
    move: (d, e) => ({ ...d, width: draggedPanelWidth(d.start, d.x, e.clientX, d.bounds) }),
    end: (d) => {
      if (d.width !== null) keep(d.width);
    },
  });
  const onPointerDown = (e: Pointer) => {
    if (!panel.current) return;
    const area = panel.current.parentElement?.getBoundingClientRect().width ?? 0;
    press(e, {
      x: e.clientX,
      start: panel.current.getBoundingClientRect().width,
      bounds: bounds(area),
      width: null,
    });
  };
  return {
    dragged: held?.width ?? null,
    handlers: { onPointerDown, ...handlers, onDoubleClick: () => keep(null) },
  };
}

/** The edge's handlers and look, for `PanelEdge`; `null` at full size,
 *  where there is no edge to drag. */
export type PanelEdgeProps = EdgeHandlers & { className: string };

/** A trace's or a session's panel: its width and whether it fills the content
 *  area, as the person set them, its left edge dragged. */
export function usePanelWidth(panel: RefObject<HTMLDivElement | null>): {
  full: boolean;
  width: number | null;
  toggleFull: () => void;
  edge: PanelEdgeProps | null;
} {
  const { peekFull, peekWidth } = useTracePrefs();
  const { dragged, handlers } = useDraggedEdge(
    panel,
    (area) => ({ grows: "left", min: Math.min(PEEK_WIDTH_MIN, area), max: area }),
    (width) => setTracePrefs({ peekWidth: width }),
  );
  return {
    full: peekFull,
    width: dragged ?? peekWidth,
    toggleFull: () => setTracePrefs({ peekFull: !peekFull }),
    edge: peekFull
      ? null
      : {
          ...handlers,
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

/** The filters' edge: its handlers, the width it stands at and whether it
 *  is being dragged, for `FiltersEdge`. */
export type FiltersEdgeProps = EdgeHandlers & {
  onKeyDown: (e: ReactKeyboardEvent<HTMLDivElement>) => void;
  now: number;
  dragging: boolean;
};

/** The filters' width, as the person dragged or keyed it, `null` for their
 *  own, and the edge between them and the list: a splitter the keyboard
 *  moves too, the filters having no Full size to fall back on. */
export function useFiltersWidth(panel: RefObject<HTMLElement | null>): {
  width: number | null;
  edge: FiltersEdgeProps;
} {
  const { filtersWidth } = useTracePrefs();
  const keep = (width: number | null) => setTracePrefs({ filtersWidth: width });
  const { dragged, handlers } = useDraggedEdge(panel, filtersBounds, keep);
  const width = dragged ?? filtersWidth;
  const now = width ?? FILTERS_WIDTH_DEFAULT;
  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      keep(null);
      return;
    }
    const { min, max } = filtersBounds(panel.current?.parentElement?.getBoundingClientRect().width ?? 0);
    const to = {
      ArrowRight: now + FILTERS_WIDTH_STEP,
      ArrowLeft: now - FILTERS_WIDTH_STEP,
      Home: min,
      End: max,
    }[e.key];
    if (to === undefined) return;
    e.preventDefault();
    keep(Math.min(Math.max(Math.round(to), min), max));
  };
  return { width, edge: { ...handlers, onKeyDown, now, dragging: dragged !== null } };
}

/** The edge between the filters and the list, in the middle of the gap
 *  between them: a separator the pointer drags and the keyboard moves. */
export function FiltersEdge({ edge }: { edge: FiltersEdgeProps }) {
  const { now, dragging, ...handlers } = edge;
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize the filters"
      aria-valuemin={FILTERS_WIDTH_MIN}
      aria-valuemax={FILTERS_WIDTH_MAX}
      aria-valuenow={now}
      tabIndex={0}
      data-testid="filters-resize"
      {...handlers}
      className={cn(
        // 6px wide, 5px from the filters and from the list (`gap-4`).
        "absolute inset-y-0 -right-[11px] z-10 hidden w-1.5 cursor-ew-resize touch-none rounded-full outline-none md:block",
        "hover:bg-foreground/10 focus-visible:bg-foreground/20",
        dragging && "bg-foreground/10",
      )}
    />
  );
}
