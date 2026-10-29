import { z } from "zod";

import { readLayoutChoice, type LayoutChoice } from "@/lib/layout";
import { PEEK_WIDTH_MIN } from "@/lib/peek";
import { DEFAULT_SESSION_COLUMNS, orderedSessionColumns, type SessionColumn } from "@/lib/sessions";
import { DEFAULT_COLUMNS, orderedColumns, type ListName } from "@/lib/traces";
import type { TraceColumn } from "@/lib/types/enums";
import { DEFAULT_USER_COLUMNS, orderedUserColumns, type UserColumn } from "@/lib/users";

// What a person chose of the lists' look, kept in their own browser: the
// columns each list shows, and for every list how tightly the rows sit, how
// often it reads itself again, whether the filters stand beside it and how
// wide; how wide a trace opened from it stands, and the layout they last
// chose for a session's page and a trace's. None of it is a list's state — a
// link a colleague opens shows their own columns — so none of it is in the
// URL; a saved view carries its columns with it, and applying one sets them.

/** How tightly the rows sit. */
export const DENSITIES = ["comfortable", "compact"] as const;
export type Density = (typeof DENSITIES)[number];

/** How often, in seconds, the page reads itself again; 0 is never. */
export const REFRESH_INTERVALS = [0, 30, 60, 300, 900] as const;
export type RefreshSeconds = (typeof REFRESH_INTERVALS)[number];

export type TracePrefs = {
  columns: readonly TraceColumn[];
  /** The sessions table's columns. */
  sessionColumns: readonly SessionColumn[];
  /** The users table's. */
  userColumns: readonly UserColumn[];
  density: Density;
  refresh: RefreshSeconds;
  /** Whether the filters' sidebar shows. */
  filters: boolean;
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
  sessionColumns: DEFAULT_SESSION_COLUMNS,
  userColumns: DEFAULT_USER_COLUMNS,
  density: "comfortable",
  refresh: 0,
  filters: true,
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

/** A list of names as the browser kept it, whatever else it put among them. */
const Strings = z.array(z.unknown()).transform((v) => v.filter((c): c is string => typeof c === "string"));

/** What the browser kept, each choice read alone: a value this build does
 *  not know — an older spelling, a column since renamed, storage edited by
 *  hand — falls back to the default rather than being applied blind. */
const PrefsSchema = z.object({
  columns: Strings.transform((v) => orderedColumns(v)).catch([...DEFAULT_PREFS.columns]),
  sessionColumns: Strings.transform((v) => orderedSessionColumns(v)).catch([
    ...DEFAULT_PREFS.sessionColumns,
  ]),
  userColumns: Strings.transform((v) => orderedUserColumns(v)).catch([...DEFAULT_PREFS.userColumns]),
  density: z.enum(DENSITIES).catch(DEFAULT_PREFS.density),
  refresh: z.literal(REFRESH_INTERVALS).catch(DEFAULT_PREFS.refresh),
  filters: z.boolean().catch(DEFAULT_PREFS.filters),
  peekFull: z.boolean().catch(DEFAULT_PREFS.peekFull),
  // Optional before the transform: a transform alone makes the key required.
  peekWidth: z.unknown().optional().transform((v) => keptWidth(v)),
  layout: z.unknown().optional().transform((v) => readLayoutChoice(v)),
});

export function readPrefs(stored: string | null): TracePrefs {
  if (!stored) return DEFAULT_PREFS;
  let raw: unknown;
  try {
    raw = JSON.parse(stored);
  } catch {
    return DEFAULT_PREFS;
  }
  const parsed = PrefsSchema.safeParse(raw);
  return parsed.success ? parsed.data : DEFAULT_PREFS;
}

/** Where each list's columns are kept. */
export const COLUMNS_PREF = {
  traces: "columns",
  sessions: "sessionColumns",
  users: "userColumns",
} as const satisfies Record<ListName, keyof TracePrefs>;

/** How often the page reads itself again, in words. */
export function refreshLabel(seconds: RefreshSeconds): string {
  if (seconds === 0) return "Off";
  if (seconds < 60) return `${seconds} s`;
  return `${seconds / 60} min`;
}
