import { SessionSort, SortDirection } from "@/lib/types/enums";

import { listHref } from "./list-query";
import { listSpec, type ColumnSpec, type ListTable } from "./traces";

// The sessions list's URL is its state, as the trace list's is, and it is
// built from the same parts: its filters, the sort, the window and the page.
// A session is the traces that share the id the application gave them — not
// a sign-in session, which `lib/server/entities/active-sessions.ts` reads.

/** The columns the sessions list and the users list (`users.ts`) share, each
 *  what a row sums over its traces. */
export const SummedColumn = {
  Agents:       "agents",
  LastSeen:     "last_seen",
  Traces:       "traces",
  FailedTraces: "failed_traces",
  Duration:     "duration",
  Tokens:       "tokens",
  Spend:        "spend",
  FirstSeen:    "first_seen",
  Tags:         "tags",
} as const;
export type SummedColumn = typeof SummedColumn[keyof typeof SummedColumn];

/** A column of the sessions list a person shows or hides, by the word their
 *  browser keeps it under (`lib/trace-prefs.ts`). */
export const SessionColumn = {
  ...SummedColumn,
  User:   "user",
  Scores: "scores",
} as const;
export type SessionColumn = typeof SessionColumn[keyof typeof SessionColumn];

/** The session's id, which every row shows, since it opens the session's page. */
export const SESSION_ID_COLUMN = {
  label: "Session",
  hint: "The session's id as stored, hashed under the project's own key, which opens the session's page. Always shown.",
} as const;

/** A session's own page over `days`, the page's own where none are named. */
export function sessionHref(organization: string, project: string, sessionId: string, days?: number): string {
  return listHref(organization, project, `/sessions/${encodeURIComponent(sessionId)}`, { days });
}

/** Every column but the session's id, in the table's order: what it was and
 *  how it stands first, then what it took. */
export const SESSION_COLUMNS: readonly ColumnSpec<SessionColumn, SessionSort>[] = [
  {
    key: SessionColumn.Agents,
    label: "Agent",
    hint: "The agents whose traces the session holds.",
  },
  {
    key: SessionColumn.LastSeen,
    label: "Last trace",
    sort: SessionSort.LastSeen,
    first: SortDirection.Desc,
    hint: "When its last trace in the window ended, and how the one that started last stands.",
  },
  {
    key: SessionColumn.Traces,
    label: "Traces",
    sort: SessionSort.Traces,
    first: SortDirection.Desc,
    numeric: true,
    hint: "How many traces the session holds in the window.",
  },
  {
    key: SessionColumn.FailedTraces,
    label: "Failed",
    sort: SessionSort.FailedTraces,
    first: SortDirection.Desc,
    numeric: true,
    hint: "How many of its traces ended in error.",
  },
  {
    key: SessionColumn.Duration,
    label: "Duration",
    sort: SessionSort.Duration,
    first: SortDirection.Desc,
    numeric: true,
    hint: "How long its traces ran, summed: never the time between them.",
  },
  {
    key: SessionColumn.Tokens,
    label: "Tokens",
    sort: SessionSort.Tokens,
    first: SortDirection.Desc,
    numeric: true,
    hint: "Its traces' input and output tokens, summed.",
  },
  {
    key: SessionColumn.Spend,
    label: "Spend",
    sort: SessionSort.Spend,
    first: SortDirection.Desc,
    numeric: true,
    hint: "What its traces' model calls cost, each as the trace list prices it.",
  },
  {
    key: SessionColumn.FirstSeen,
    label: "Started",
    sort: SessionSort.FirstSeen,
    first: SortDirection.Asc,
    hint: "When its first trace in the window started.",
  },
  {
    key: SessionColumn.User,
    label: "User",
    hint: "The end user the session was with, as stored: hashed under the project's own key. It opens the user's page.",
  },
  // Before the tags, which run long: a score is seen without scrolling to it.
  {
    key: SessionColumn.Scores,
    label: "Scores",
    hint: "The session's own scores, each name's mean, or a category's values: never its traces'.",
  },
  {
    key: SessionColumn.Tags,
    label: "Tags",
    hint: "Its traces' tags, each key:value once.",
  },
];

/** The columns the list shows until a person picks others: what a session
 *  was, what it took and how it was scored, and no id stored as a hash but
 *  its own. */
export const DEFAULT_SESSION_COLUMNS: readonly SessionColumn[] = [
  SessionColumn.Agents,
  SessionColumn.LastSeen,
  SessionColumn.Traces,
  SessionColumn.Duration,
  SessionColumn.Tokens,
  SessionColumn.Spend,
  SessionColumn.Scores,
  SessionColumn.Tags,
];

/** The sessions list's columns menu. */
export const SESSION_TABLE: ListTable<SessionColumn, SessionSort> = {
  id: SESSION_ID_COLUMN,
  columns: SESSION_COLUMNS,
  defaults: DEFAULT_SESSION_COLUMNS,
};

/** The sessions list: its table, menu, links and sorts; the server's default
 *  is the session last heard from first. */
export const SESSIONS = listSpec<SessionColumn, SessionSort>({
  path: "/sessions",
  table: SESSION_TABLE,
  sorts: SessionSort,
  defaultSort: SessionSort.LastSeen,
  presets: [
    { id: "errors", label: "Errors", hint: "Only the sessions holding a trace that ended in error" },
    { id: "slow", label: "Slow", hint: "The sessions whose traces ran longest first" },
    { id: "tokens", label: "Tokens", hint: "The sessions that used the most tokens first" },
  ],
  presetSorts: { slow: SessionSort.Duration, tokens: SessionSort.Tokens },
});

export const parseSessionQuery = SESSIONS.parse;
export const sessionsHref = SESSIONS.href;
