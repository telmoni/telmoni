import { z } from "zod";

import { PEEK_WIDTH_MIN } from "@/lib/peek";
import { DEFAULT_SESSION_COLUMNS, SESSIONS, type SessionColumn } from "@/lib/sessions";
import { DEFAULT_COLUMNS, orderedColumns, type ListName } from "@/lib/traces";
import type { TraceColumn } from "@/lib/types/traces";
import { DEFAULT_USER_COLUMNS, USERS, type UserColumn } from "@/lib/users";

// What a person chose of the lists' look, kept in their own browser: the
// columns each shows, how tightly the rows sit (set on the General settings
// page), whether the filters stand beside it, how wide and how many of them,
// and how wide a trace opened from it stands. None of it is a list's state —
// a link a colleague opens shows their own columns — so none of it is in the
// URL.

/** How tightly the rows sit. */
export const DENSITIES = ["comfortable", "compact"] as const;
export type Density = (typeof DENSITIES)[number];

export type TracePrefs = {
  columns: readonly TraceColumn[];
  sessionColumns: readonly SessionColumn[];
  userColumns: readonly UserColumn[];
  density: Density;
  /** Whether the filters' sidebar shows. */
  filters: boolean;
  /** Whether the sidebar shows every filter, past the ones it opens with. */
  moreFilters: boolean;
  /** Whether a trace opened from the list fills the content area, rather
   *  than standing beside the list (`components/traces/peek.tsx`). */
  peekFull: boolean;
  /** How wide, in pixels, the person dragged that panel beside the list;
   *  `null` for its own width. */
  peekWidth: number | null;
};

export const DEFAULT_PREFS: TracePrefs = {
  columns: DEFAULT_COLUMNS,
  sessionColumns: DEFAULT_SESSION_COLUMNS,
  userColumns: DEFAULT_USER_COLUMNS,
  density: "comfortable",
  filters: true,
  moreFilters: false,
  peekFull: false,
  peekWidth: null,
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
  sessionColumns: Strings.transform((v) => SESSIONS.ordered(v)).catch([...DEFAULT_PREFS.sessionColumns]),
  userColumns: Strings.transform((v) => USERS.ordered(v)).catch([...DEFAULT_PREFS.userColumns]),
  density: z.enum(DENSITIES).catch(DEFAULT_PREFS.density),
  filters: z.boolean().catch(DEFAULT_PREFS.filters),
  moreFilters: z.boolean().catch(DEFAULT_PREFS.moreFilters),
  peekFull: z.boolean().catch(DEFAULT_PREFS.peekFull),
  // Optional before the transform: a transform alone makes the key required.
  peekWidth: z.unknown().optional().transform((v) => keptWidth(v)),
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
