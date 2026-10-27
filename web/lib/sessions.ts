import type { SessionQuery, SessionTrace } from "@/lib/server/entities/sessions";
import { SessionSort, SortDirection } from "@/lib/types/enums";

import {
  listHref,
  sortAfterClick,
  sortInForce,
  type Column,
  type SearchParams,
  type WindowQuery,
} from "./list-query";
import { columnMenu, ordered, parseListQuery, sortsOf, type ColumnSpec, type ListTable } from "./traces";

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

/** The sessions list's columns menu, in its rows. */
export const SESSION_COLUMN_MENU = columnMenu(SESSION_COLUMNS);

/** `columns` as the sessions table shows them. */
export function orderedSessionColumns(columns: readonly string[]): SessionColumn[] {
  return ordered(SESSION_COLUMNS, columns);
}

/** Every sort the list takes, the id's among them. */
const SORTS: readonly Column<SessionSort>[] = [
  { sort: SessionSort.Session, label: SESSION_ID_COLUMN.label, first: SortDirection.Asc },
  ...sortsOf(SESSION_COLUMNS),
];

// The server's default: the session last heard from first.
const DEFAULT_SORT = SessionSort.LastSeen;

/** The query the URL carries, every value the server would refuse dropped. */
export function parseSessionQuery(searchParams: SearchParams): SessionQuery {
  return parseListQuery(searchParams, SessionSort);
}

/** The list's path with `query` in its URL: `patch` changes some keys, and a
 *  key patched to `undefined` is dropped. */
export function sessionsHref(
  organization: string,
  project: string,
  query: SessionQuery,
  patch: Partial<SessionQuery> = {},
): string {
  return listHref(organization, project, "/sessions", query, patch);
}

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

/** The sort in force: the URL's, else the server's default. */
export function currentSessionSort(query: SessionQuery): {
  sort: SessionSort;
  dir: SortDirection;
} {
  return sortInForce(SORTS, DEFAULT_SORT, query);
}

/** What clicking a column heading asks for, from the first page. */
export function nextSessionSort(
  query: SessionQuery,
  column: SessionSort,
): { sort: SessionSort; dir: SortDirection; page: undefined } {
  return { ...sortAfterClick(SORTS, DEFAULT_SORT, query, column), page: undefined };
}

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
