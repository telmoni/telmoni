import { readLayoutChoice, type LayoutChoice } from "@/lib/layout";
import { keptFiltersWidth } from "@/lib/panel-width";
import { PEEK_WIDTH_MIN } from "@/lib/peek";
import { DEFAULT_COLUMNS, orderedColumns } from "@/lib/traces";
import type { TraceColumn } from "@/lib/types/enums";

// What a person chose of the trace list's look, kept in their own browser as
// Langfuse keeps a table's columns: which columns show, how tightly the rows
// sit, how often the page reads itself again, whether the filters stand
// beside it and how wide, how wide a trace opened from it stands, full size or dragged,
// and the layout they last chose for a session's page and a trace's. None
// of it is the list's state — a link a colleague opens shows their own
// columns — so none of it is in the URL; a saved view carries its columns
// with it, and applying one sets them here.

/** How tightly the rows sit. */
export const DENSITIES = ["comfortable", "compact"] as const;
export type Density = (typeof DENSITIES)[number];

/** How often, in seconds, the page reads itself again; 0 is never. */
export const REFRESH_INTERVALS = [0, 30, 60, 300, 900] as const;
export type RefreshSeconds = (typeof REFRESH_INTERVALS)[number];

export type TracePrefs = {
  columns: readonly TraceColumn[];
  density: Density;
  refresh: RefreshSeconds;
  /** Whether the filters' sidebar shows. */
  filters: boolean;
  /** How wide, in pixels, the person dragged the filters; `null` for their
   *  own width. */
  filtersWidth: number | null;
  /** Whether a trace opened from the list fills the content area, rather
   *  than standing beside the list (`components/traces/peek.tsx`). */
  peekFull: boolean;
  /** How wide, in pixels, the person dragged that panel beside the list;
   *  `null` for its own width. */
  peekWidth: number | null;
  /** The layout a session's page and a trace's last opened in, with its
   *  options (`lib/layout.ts`); `null` until one is chosen, when the
   *  project's default holds. */
  layout: LayoutChoice | null;
};

export const DEFAULT_PREFS: TracePrefs = {
  columns: DEFAULT_COLUMNS,
  density: "comfortable",
  refresh: 0,
  filters: true,
  filtersWidth: null,
  peekFull: false,
  peekWidth: null,
  layout: null,
};

/** A dragged width this build keeps: whole pixels, no narrower than a drag
 *  leaves the panel. One wider than the area the panel stands in is held to
 *  the area as it is drawn. */
function keptWidth(value: unknown): number | null {
  return typeof value === "number" &&
    Number.isInteger(value) &&
    value >= PEEK_WIDTH_MIN &&
    value <= 10_000
    ? value
    : null;
}

/** Where the browser keeps them. */
export const TRACE_PREFS_KEY = "telmoni-traces";

/** What the browser kept, read: a value this build does not know — an older
 *  spelling, a column since renamed, storage edited by hand — falls back to
 *  the default rather than being applied blind. */
export function readPrefs(stored: string | null): TracePrefs {
  if (!stored) return DEFAULT_PREFS;
  let raw: unknown;
  try {
    raw = JSON.parse(stored);
  } catch {
    return DEFAULT_PREFS;
  }
  if (typeof raw !== "object" || raw === null) return DEFAULT_PREFS;
  const kept = raw as Record<string, unknown>;
  const columns = Array.isArray(kept.columns)
    ? orderedColumns(kept.columns.filter((c): c is string => typeof c === "string"))
    : DEFAULT_PREFS.columns;
  return {
    columns,
    density: DENSITIES.includes(kept.density as Density)
      ? (kept.density as Density)
      : DEFAULT_PREFS.density,
    refresh: REFRESH_INTERVALS.includes(kept.refresh as RefreshSeconds)
      ? (kept.refresh as RefreshSeconds)
      : DEFAULT_PREFS.refresh,
    filters: typeof kept.filters === "boolean" ? kept.filters : DEFAULT_PREFS.filters,
    filtersWidth: keptFiltersWidth(kept.filtersWidth),
    peekFull: typeof kept.peekFull === "boolean" ? kept.peekFull : DEFAULT_PREFS.peekFull,
    peekWidth: keptWidth(kept.peekWidth),
    layout: readLayoutChoice(kept.layout),
  };
}

/** How often the page reads itself again, in words. */
export function refreshLabel(seconds: RefreshSeconds): string {
  if (seconds === 0) return "Off";
  if (seconds < 60) return `${seconds} s`;
  return `${seconds / 60} min`;
}
