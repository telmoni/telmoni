"use client";

import { useEffect, useId, useState, type PointerEvent, type ReactNode, type RefObject } from "react";
import { Maximize2, Minimize2, X as XIcon } from "lucide-react";

import { PanelAction } from "@/components/side-panel";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useNonModal } from "@/components/use-non-modal";
import { usePointerGesture } from "@/components/use-pointer-gesture";
import {
  floatingRect,
  fullRect,
  isCompact,
  moveRect,
  readWindowLayout,
  resizeRect,
  writeWindowLayout,
  type Edge,
  type Rect,
  type Viewport,
  type WindowLayout,
} from "@/lib/agent/window";
import { cn } from "@/lib/utils";

// Each edge and corner a pointer can size the window by, straddling the edge
// as a desktop's do, so neither a press on the edge nor one just outside it
// misses. The right edge lies wholly outside: inside it is the conversation's
// scrollbar, which keeps its presses. A corner is the larger target, and the
// header's and composer's gutter keeps every control clear of all four.
const HANDLES: { edge: Edge; className: string }[] = [
  { edge: "n", className: "inset-x-3 -top-1 h-2 cursor-ns-resize" },
  { edge: "s", className: "inset-x-3 -bottom-1 h-2 cursor-ns-resize" },
  { edge: "e", className: "inset-y-3 -right-1.5 w-1.5 cursor-ew-resize" },
  { edge: "w", className: "inset-y-3 -left-1 w-2 cursor-ew-resize" },
  { edge: "nw", className: "-top-1.5 -left-1.5 size-4 cursor-nwse-resize" },
  { edge: "se", className: "-right-1.5 -bottom-1.5 size-4 cursor-nwse-resize" },
  { edge: "ne", className: "-top-1.5 -right-1.5 size-4 cursor-nesw-resize" },
  { edge: "sw", className: "-bottom-1.5 -left-1.5 size-4 cursor-nesw-resize" },
];

/** What a drag of the window holds: the edge it sizes by, or the whole of it
 *  moved; where the press was and the window then; and where it has the
 *  window so far, `null` until the pointer moves. */
type Gesture = { kind: "move" | Edge; x: number; y: number; start: Rect; rect: Rect | null };

// The layout viewport, which everything is measured in, and the part of it a
// phone's keyboard leaves visible: the visual viewport's top and height.
interface Viewports extends Viewport {
  visibleTop: number;
  visibleHeight: number;
}

const readViewports = (): Viewports => {
  const root = document.documentElement;
  const visible = window.visualViewport;
  return {
    width: root.clientWidth,
    height: root.clientHeight,
    visibleTop: visible ? visible.offsetTop : 0,
    visibleHeight: visible ? visible.height : root.clientHeight,
  };
};

// Mounted only while the window is open, which is only ever in the browser.
function useViewports(): Viewports {
  const [viewports, setViewports] = useState(readViewports);
  useEffect(() => {
    let frame = 0;
    const onChange = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => setViewports(readViewports()));
    };
    window.addEventListener("resize", onChange);
    window.visualViewport?.addEventListener("resize", onChange);
    window.visualViewport?.addEventListener("scroll", onChange);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", onChange);
      window.visualViewport?.removeEventListener("resize", onChange);
      window.visualViewport?.removeEventListener("scroll", onChange);
    };
  }, []);
  return viewports;
}

// The console header's bottom edge, which a banner above it moves down.
const headerBottom = () =>
  document.querySelector('[data-slot="console-header"]')?.getBoundingClientRect().bottom;

/**
 * The agent's window: a dialog that floats over the console without shutting
 * it off, moved by its title bar, sized from any edge or corner, and made to
 * fill the screen and back from its header or by a double press on the title
 * bar. Where it was left, and whether it filled the screen, outlives the page
 * (`lib/agent/window.ts`). A screen too small to float on gets it full size,
 * with nothing to drag.
 *
 * ⚠ **Non-modal, and marked so: `aria-modal="false"`.** The page beside it
 * keeps its pointer and its keys — the letter shortcuts and the rail's drawer
 * read that attribute to tell this window from a dialog that does shut the
 * page off (`MODAL_SELECTOR` in `lib/keys.ts`). It takes the focus as it
 * opens and shuts on Escape from inside, as the side panel does.
 *
 * The panel's state lives in `AgentPanel`, which hands the conversation in as
 * `children`: a drag re-renders this frame alone, never the conversation.
 */
export function AgentWindow({
  id,
  ref,
  title,
  actions,
  initialFocus,
  onClose,
  children,
}: {
  id: string;
  ref: RefObject<HTMLDivElement | null>;
  title: ReactNode;
  /** The panel's own controls, before full size and close. */
  actions: ReactNode;
  /** What takes the focus as the window opens; the window itself if none. */
  initialFocus?: RefObject<HTMLElement | null>;
  onClose: () => void;
  children: ReactNode;
}) {
  const titleId = useId();
  const viewports = useViewports();
  const viewport: Viewport = { width: viewports.width, height: viewports.height };
  const [below] = useState(headerBottom);
  const [layout, setLayout] = useState<WindowLayout>(readWindowLayout);
  useNonModal(ref, { onClose, initialFocus });

  const compact = isCompact(viewport);
  const full = compact || layout.mode === "full";
  // ⚠ **A phone's window fills what its keyboard leaves**, not the layout
  // viewport: the keyboard covers the bottom of that, and the composer with it.
  const rect = compact
    ? { x: 0, y: viewports.visibleTop, width: viewports.width, height: viewports.visibleHeight }
    : full
      ? fullRect(viewport)
      : floatingRect(layout, viewport, below);

  // Kept as a drag ends and as the window fills the screen or floats again:
  // one write per gesture, not one per pointer move.
  const keep = (next: WindowLayout) => {
    setLayout(next);
    writeWindowLayout(next);
  };
  const toggleFull = () => keep({ ...layout, mode: layout.mode === "full" ? "float" : "full" });

  const {
    held: gesture,
    press,
    handlers: gestureHandlers,
  } = usePointerGesture<Gesture>({
    move: (g, e) => {
      const dx = e.clientX - g.x;
      const dy = e.clientY - g.y;
      const next =
        g.kind === "move"
          ? moveRect(g.start, dx, dy, viewport)
          : resizeRect(g.start, g.kind, dx, dy, viewport);
      setLayout({ mode: "float", rect: next });
      return { ...g, rect: next };
    },
    end: (g) => {
      if (g.rect) writeWindowLayout({ mode: "float", rect: g.rect });
    },
  });

  const begin = (kind: Gesture["kind"], e: PointerEvent<HTMLElement>) => {
    if (full) return;
    // A press on one of the title bar's controls is the control's.
    if (kind === "move" && (e.target as Element).closest("button, a")) return;
    press(e, { kind, x: e.clientX, y: e.clientY, start: rect, rect: null });
  };

  return (
    <div
      className={cn(
        "fixed z-50 animate-in fade-in-0 zoom-in-95",
        // Moving between floating and full size glides; a drag or a resize
        // follows the pointer exactly, so nothing eases behind it then. The
        // duration goes with the property list: alone, it would ease every
        // property, the drag's included.
        gesture === null && "transition-[left,top,width,height] duration-200 ease-out",
      )}
      style={{ left: rect.x, top: rect.y, width: rect.width, height: rect.height }}
    >
      <div
        ref={ref}
        id={id}
        role="dialog"
        aria-modal="false"
        aria-labelledby={titleId}
        tabIndex={-1}
        data-mode={full ? "full" : "float"}
        className={cn(
          "group/agent flex size-full flex-col overflow-hidden bg-card text-card-foreground outline-none",
          !compact && "rounded-lg border border-border shadow-menu",
        )}
      >
        <div
          onPointerDown={(e) => begin("move", e)}
          onDoubleClick={(e) => {
            if (compact || (e.target as Element).closest("button, a")) return;
            toggleFull();
          }}
          {...gestureHandlers}
          // The title is centred on the controls, as a dialog's is on its
          // close: a dialog's title may wrap, so it sits a fixed 7px down from
          // the top instead, which a touch screen's 44px buttons put off
          // centre. This one is a single line, cut short rather than wrapped.
          // The bar's inset is its own on every side, so a scrolled
          // conversation stops a gutter under the controls rather than running
          // beneath them.
          className={cn(
            "flex shrink-0 items-center gap-3 p-gutter select-none",
            !full && "touch-none",
            !full && (gesture?.kind === "move" ? "cursor-grabbing" : "cursor-grab"),
          )}
        >
          <h2
            id={titleId}
            className="flex min-w-0 items-center gap-2 text-base leading-snug font-semibold"
          >
            {title}
          </h2>
          <TooltipProvider>
            <div className="ml-auto flex shrink-0 items-center gap-1">
              {actions}
              {!compact && (
                <PanelAction label={full ? "Exit full size" : "Full size"} onClick={toggleFull}>
                  {full ? <Minimize2 className="size-4" /> : <Maximize2 className="size-4" />}
                </PanelAction>
              )}
              <PanelAction label="Close the agent" onClick={onClose}>
                <XIcon className="size-4" />
              </PanelAction>
            </div>
          </TooltipProvider>
        </div>
        {children}
      </div>
      {!full &&
        HANDLES.map((h) => (
          <div
            key={h.edge}
            aria-hidden
            onPointerDown={(e) => begin(h.edge, e)}
            {...gestureHandlers}
            className={cn("absolute touch-none", h.className)}
          />
        ))}
    </div>
  );
}
