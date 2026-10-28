import type { SessionTrace } from "@/lib/server/entities/sessions";
import { SessionSort, SortDirection } from "@/lib/types/enums";

import { listHref, type WindowQuery } from "./list-query";
import { listSpec, type ColumnSpec, type ListTable } from "./traces";

// The sessions list's URL is its state, as the trace list's is: its filters,
// the sort, the window and the page, read with every value the server would
// refuse dropped, and written with one thing changed. A session is the runs
// that share the id the application gave them — not a sign-in session, which
// `lib/server/entities/active-sessions.ts` reads.

/** A column of the sessions table a person may show or hide. */
export type SessionColumn =
  | "user"
  | "traces"
  | "errors"
  | "steps"
  | "input_tokens"
  | "output_tokens"
  | "tokens"
  | "first_seen"
  | "last_seen";

/** The session's id, which every row shows, since it opens the session. */
export const SESSION_ID_COLUMN = {
  label: "Session",
  hint: "The session's id, as the application set it, which opens the session beside the list. Always shown.",
} as const;

/** Every column but the session's id, in the table's order, each with the
 *  way it sorts when first clicked: ids and first seen the alphabet's and the
 *  calendar's way, the most and the latest first on the others. */
export const SESSION_COLUMNS: readonly ColumnSpec<SessionColumn, SessionSort>[] = [
  {
    key: "user",
    label: "User",
    sort: SessionSort.User,
    first: SortDirection.Asc,
    hint: "The end user the session was with, as stored: hashed under the project's own key.",
  },
  {
    key: "traces",
    label: "Traces",
    sort: SessionSort.Traces,
    first: SortDirection.Desc,
    numeric: true,
    group: "counts",
    hint: "How many runs the session had in the window.",
  },
  {
    key: "errors",
    label: "Errors",
    sort: SessionSort.Errors,
    first: SortDirection.Desc,
    numeric: true,
    group: "counts",
    hint: "How many of its runs had a span that failed.",
  },
  {
    key: "steps",
    label: "Steps",
    sort: SessionSort.Steps,
    first: SortDirection.Desc,
    numeric: true,
    group: "counts",
  },
  {
    key: "input_tokens",
    label: "Input tokens",
    sort: SessionSort.InputTokens,
    first: SortDirection.Desc,
    numeric: true,
    group: "tokens",
  },
  {
    key: "output_tokens",
    label: "Output tokens",
    sort: SessionSort.OutputTokens,
    first: SortDirection.Desc,
    numeric: true,
    group: "tokens",
  },
  {
    key: "tokens",
    label: "Tokens",
    sort: SessionSort.Tokens,
    first: SortDirection.Desc,
    numeric: true,
    group: "tokens",
    hint: "Input and output tokens, summed.",
  },
  {
    key: "first_seen",
    label: "First seen",
    sort: SessionSort.FirstSeen,
    first: SortDirection.Asc,
    hint: "When the session's first run in the window started.",
  },
  {
    key: "last_seen",
    label: "Last seen",
    sort: SessionSort.LastSeen,
    first: SortDirection.Desc,
    hint: "When its last run in the window ended.",
  },
];

/** The columns the table shows until a person picks others: what it showed
 *  before it offered the rest. */
export const DEFAULT_SESSION_COLUMNS: readonly SessionColumn[] = [
  "user",
  "traces",
  "errors",
  "steps",
  "tokens",
  "first_seen",
  "last_seen",
];

/** The sessions list's columns menu. */
export const SESSION_TABLE: ListTable<SessionColumn, SessionSort> = {
  id: SESSION_ID_COLUMN,
  columns: SESSION_COLUMNS,
  defaults: DEFAULT_SESSION_COLUMNS,
};

/** The sessions list: its table, menu, links and sorts, the id's among them;
 *  the server's default is the session last heard from first. */
export const SESSIONS = listSpec({
  path: "/sessions",
  table: SESSION_TABLE,
  sorts: SessionSort,
  idSort: { sort: SessionSort.Session, label: SESSION_ID_COLUMN.label, first: SortDirection.Asc },
  defaultSort: SessionSort.LastSeen,
});

export const SESSION_COLUMN_MENU = SESSIONS.menu;
export const orderedSessionColumns = SESSIONS.ordered;
export const parseSessionQuery = SESSIONS.parse;
export const sessionsHref = SESSIONS.href;

/** A session's own page, by its id, which the path carries URL-encoded: it is
 *  up to 200 characters of the application's choosing, and Next hands the
 *  page the segment decoded again. */
export function sessionHref(
  organization: string,
  project: string,
  sessionId: string,
  query: WindowQuery = {},
): string {
  return listHref(organization, project, `/sessions/${encodeURIComponent(sessionId)}`, query);
}

/** A session's traces in a window, through the console's own route, each
 *  with the spans `pinned` names. */
export function sessionReadHref(
  sessionId: string,
  projectId: string,
  query: WindowQuery,
  pinned: readonly string[],
): string {
  const params = new URLSearchParams({ project: projectId });
  if (query.days !== undefined) params.set("days", String(query.days));
  if (query.limit !== undefined) params.set("limit", String(query.limit));
  for (const name of pinned) params.append("pinned", name);
  return `/api/v1/sessions/${encodeURIComponent(sessionId)}?${params}`;
}

export const currentSessionSort = SESSIONS.current;
export const nextSessionSort = SESSIONS.next;

/** What a session's traces add up to, as its page and its panel show them:
 *  the user the first run that named one was with; the runs with a span that
 *  failed, a run that recovered from a failed call among them, as the list
 *  counts; their steps and tokens; and when the last run ended. */
export function sessionSums(traces: readonly SessionTrace[]): {
  user: string;
  errors: number;
  steps: number;
  inputTokens: number;
  outputTokens: number;
  lastEnded: string;
} {
  return {
    user: traces.find((t) => t.user_id)?.user_id ?? "",
    errors: traces.filter((t) => t.errors > 0).length,
    steps: traces.reduce((sum, t) => sum + t.steps, 0),
    inputTokens: traces.reduce((sum, t) => sum + t.input_tokens, 0),
    outputTokens: traces.reduce((sum, t) => sum + t.output_tokens, 0),
    // As times, never as text: `…:00Z` sorts after `…:00.250Z` as text.
    lastEnded: traces.reduce(
      (latest, t) => (latest === "" || Date.parse(t.ended_at) > Date.parse(latest) ? t.ended_at : latest),
      "",
    ),
  };
}
