// A panel beside a list, its edge dragged: the width it comes to as the
// pointer moves, held between the widths it may stand at. The trace and
// session panels grow leftwards from the list's right edge, the filters
// rightwards from its left.

/** Which way a panel grows as its edge is dragged, and the widths it is
 *  held between. */
export type DragBounds = { grows: "left" | "right"; min: number; max: number };

/** The panel's width as its edge is dragged from `startX` to `x`: wider the
 *  way it grows, in whole pixels, never under `min` nor over `max`, and
 *  `min` where the two cross. */
export function draggedPanelWidth(start: number, startX: number, x: number, bounds: DragBounds): number {
  const moved = bounds.grows === "left" ? startX - x : x - startX;
  return Math.round(Math.min(Math.max(start + moved, bounds.min), Math.max(bounds.max, bounds.min)));
}

/** The filters' own width: `--spacing-sidebar-inner` (`globals.css`), the
 *  console's rail rows', in pixels. */
export const FILTERS_WIDTH_DEFAULT = 164;

/** The narrowest the filters are dragged to, and the widest. */
export const FILTERS_WIDTH_MIN = 128;
export const FILTERS_WIDTH_MAX = 480;

/** How far an arrow key moves the filters' edge. */
export const FILTERS_WIDTH_STEP = 16;

/** The widths the filters may stand at beside a row `area` wide: never past
 *  half of it, so the list keeps its room; the whole range where the row is
 *  not laid out. */
export function filtersBounds(area: number): DragBounds {
  const max =
    area > 0 ? Math.max(FILTERS_WIDTH_MIN, Math.min(FILTERS_WIDTH_MAX, Math.floor(area / 2))) : FILTERS_WIDTH_MAX;
  return { grows: "right", min: FILTERS_WIDTH_MIN, max };
}

/** A kept width this build stands the filters at: whole pixels in their range. */
export function keptFiltersWidth(value: unknown): number | null {
  return typeof value === "number" &&
    Number.isInteger(value) &&
    value >= FILTERS_WIDTH_MIN &&
    value <= FILTERS_WIDTH_MAX
    ? value
    : null;
}
