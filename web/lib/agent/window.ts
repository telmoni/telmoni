import { z } from "zod";

// The agent's window: where it floats, how big it is, and whether it fills
// the screen. Geometry only, in CSS pixels from the viewport's top left, so
// the drag and resize handlers in `components/agent/agent-window.tsx` stay
// thin and every rule about where the window may go is tested here.

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Viewport {
  width: number;
  height: number;
}

export type WindowMode = "float" | "full";

export interface WindowLayout {
  mode: WindowMode;
  /** Where it floats; `null` until the person first moves or sizes it. */
  rect: Rect | null;
}

export type Edge = "n" | "e" | "s" | "w" | "ne" | "se" | "sw" | "nw";

/** The console's edge padding (`p-3.5`): full size keeps it all round, and
 *  the window first opens that far in from the right and under the header. */
export const WINDOW_EDGE_PX = 14;

// The console's header (`h-15`), for where it cannot be measured. The window
// first opens below it, clear of the button that opened it — lower when a
// banner stands above the header.
const HEADER_PX = 60;

// The least the header's four controls, the title and the composer need.
export const WINDOW_MIN = { width: 360, height: 420 } as const;

const WINDOW_DEFAULT = { width: 480, height: 720 } as const;

// Below either, a window that floats would cover most of the screen anyway,
// and a phone has no pointer to drag it with: it fills the screen instead.
const COMPACT_BELOW = { width: 640, height: 480 } as const;

export const AGENT_WINDOW_STORAGE_KEY = "telmoni-agent-window";

const FLOATING: WindowLayout = { mode: "float", rect: null };

const clamp = (n: number, lo: number, hi: number) => Math.min(Math.max(n, lo), hi);

export function isCompact(viewport: Viewport): boolean {
  return viewport.width < COMPACT_BELOW.width || viewport.height < COMPACT_BELOW.height;
}

/**
 * Kept on screen: no larger than the viewport, no smaller than the minimum
 * (or the viewport, where that is smaller), and wholly inside it, so no edge
 * or control is ever out of reach.
 */
export function clampRect(rect: Rect, viewport: Viewport): Rect {
  const width = clamp(rect.width, Math.min(WINDOW_MIN.width, viewport.width), viewport.width);
  const height = clamp(rect.height, Math.min(WINDOW_MIN.height, viewport.height), viewport.height);
  return {
    x: clamp(rect.x, 0, viewport.width - width),
    y: clamp(rect.y, 0, viewport.height - height),
    width,
    height,
  };
}

/** Where the window opens before it has been moved: at the right, under the
 *  header (whose bottom edge is `below`), as tall as the screen allows up to
 *  its default. */
export function defaultRect(viewport: Viewport, below = HEADER_PX): Rect {
  const top = below + WINDOW_EDGE_PX;
  const width = Math.min(WINDOW_DEFAULT.width, viewport.width - 2 * WINDOW_EDGE_PX);
  const height = Math.min(WINDOW_DEFAULT.height, viewport.height - top - WINDOW_EDGE_PX);
  return clampRect(
    { x: viewport.width - WINDOW_EDGE_PX - width, y: top, width, height },
    viewport,
  );
}

export function fullRect(viewport: Viewport): Rect {
  return {
    x: WINDOW_EDGE_PX,
    y: WINDOW_EDGE_PX,
    width: Math.max(0, viewport.width - 2 * WINDOW_EDGE_PX),
    height: Math.max(0, viewport.height - 2 * WINDOW_EDGE_PX),
  };
}

/** The floating window's rect on this screen. */
export function floatingRect(layout: WindowLayout, viewport: Viewport, below?: number): Rect {
  return layout.rect ? clampRect(layout.rect, viewport) : defaultRect(viewport, below);
}

/** Dragged by `dx`, `dy` from where the drag began; it stops at the edges. */
export function moveRect(start: Rect, dx: number, dy: number, viewport: Viewport): Rect {
  return clampRect({ ...start, x: start.x + dx, y: start.y + dy }, viewport);
}

/**
 * Sized by one edge or corner, dragged by `dx`, `dy` from where the drag
 * began. The edges not held stay where they were: a window sized from the
 * left keeps its right edge, as a window on a desktop does, and stops at the
 * minimum rather than sliding.
 */
export function resizeRect(
  start: Rect,
  edge: Edge,
  dx: number,
  dy: number,
  viewport: Viewport,
): Rect {
  const from = clampRect(start, viewport);
  const minWidth = Math.min(WINDOW_MIN.width, viewport.width);
  const minHeight = Math.min(WINDOW_MIN.height, viewport.height);
  const right = from.x + from.width;
  const bottom = from.y + from.height;
  let { x, y, width, height } = from;
  if (edge.includes("e")) width = clamp(from.width + dx, minWidth, viewport.width - from.x);
  if (edge.includes("w")) {
    width = clamp(from.width - dx, minWidth, right);
    x = right - width;
  }
  if (edge.includes("s")) height = clamp(from.height + dy, minHeight, viewport.height - from.y);
  if (edge.includes("n")) {
    height = clamp(from.height - dy, minHeight, bottom);
    y = bottom - height;
  }
  return { x, y, width, height };
}

const RectSchema = z.object({
  x: z.number(),
  y: z.number(),
  width: z.number().positive(),
  height: z.number().positive(),
});

const LayoutSchema = z.object({
  mode: z.enum(["float", "full"]),
  rect: RectSchema.nullable(),
});

/** The layout the person left the window in, or floating where it first
 *  opens when there is none, or none this build can read. */
export function readWindowLayout(): WindowLayout {
  let raw: string | null;
  try {
    raw = window.localStorage.getItem(AGENT_WINDOW_STORAGE_KEY);
  } catch {
    return FLOATING;
  }
  if (!raw) return FLOATING;
  let body: unknown;
  try {
    body = JSON.parse(raw);
  } catch {
    return FLOATING;
  }
  const parsed = LayoutSchema.safeParse(body);
  return parsed.success ? parsed.data : FLOATING;
}

// Storage disabled or full: the window keeps its place for as long as it is
// open, and opens where it first did next time.
export function writeWindowLayout(layout: WindowLayout): void {
  const rect = layout.rect && {
    x: Math.round(layout.rect.x),
    y: Math.round(layout.rect.y),
    width: Math.round(layout.rect.width),
    height: Math.round(layout.rect.height),
  };
  try {
    window.localStorage.setItem(AGENT_WINDOW_STORAGE_KEY, JSON.stringify({ mode: layout.mode, rect }));
  } catch {}
}
