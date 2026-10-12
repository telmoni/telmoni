import type { SessionQuery } from "@/lib/server/entities/sessions";
import { SessionSort, SortDirection } from "@/lib/types/enums";

import {
  compact,
  listHref,
  oneOf,
  parseWindow,
  sortAfterClick,
  sortInForce,
  type Column,
  type SearchParams,
  type WindowQuery,
} from "./list-query";

// The sessions list's URL is its state, as the trace list's is: the sort and
// the window, read with every value the server would refuse dropped, and
// written with one thing changed. A session is the runs that share the id the
// application gave them — not a sign-in session, which
// `lib/server/entities/active-sessions.ts` reads.

/** The columns the list sorts by, in the table's order, each with the way it
 *  sorts when first clicked: ids and first seen the alphabet's and the
 *  calendar's way, the most and the latest first on the others. */
export const SESSION_COLUMNS: readonly Column<SessionSort>[] = [
  { sort: SessionSort.Session, label: "Session", first: SortDirection.Asc },
  { sort: SessionSort.User, label: "User", first: SortDirection.Asc },
  { sort: SessionSort.Traces, label: "Traces", first: SortDirection.Desc },
  { sort: SessionSort.Errors, label: "Errors", first: SortDirection.Desc },
  { sort: SessionSort.Steps, label: "Steps", first: SortDirection.Desc },
  { sort: SessionSort.Tokens, label: "Tokens", first: SortDirection.Desc },
  { sort: SessionSort.FirstSeen, label: "First seen", first: SortDirection.Asc },
  { sort: SessionSort.LastSeen, label: "Last seen", first: SortDirection.Desc },
];

// The server's default: the session last heard from first.
const DEFAULT_SORT = SessionSort.LastSeen;

/** The query the URL carries, every value the server would refuse dropped. */
export function parseSessionQuery(searchParams: SearchParams): SessionQuery {
  return compact({
    sort: oneOf(searchParams.sort, SessionSort),
    dir: oneOf(searchParams.dir, SortDirection),
    ...parseWindow(searchParams),
  });
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
  return sortInForce(SESSION_COLUMNS, DEFAULT_SORT, query);
}

/** What clicking a column heading asks for. */
export function nextSessionSort(
  query: SessionQuery,
  column: SessionSort,
): { sort: SessionSort; dir: SortDirection } {
  return sortAfterClick(SESSION_COLUMNS, DEFAULT_SORT, query, column);
}
