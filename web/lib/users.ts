import type { UserQuery } from "@/lib/server/entities/users";
import { SortDirection, UserSort } from "@/lib/types/enums";

import {
  compact,
  listHref,
  oneOf,
  parseWindow,
  sortAfterClick,
  sortInForce,
  type Column,
  type SearchParams,
} from "./list-query";

// The users list's URL is its state, as the trace list's is: the sort and
// the window, read with every value the server would refuse dropped, and
// written with one thing changed.

/** The columns the list sorts by, in the table's order, each with the way it
 *  sorts when first clicked: ids and first seen the alphabet's and the
 *  calendar's way, the most and the latest first on the others. */
export const USER_COLUMNS: readonly Column<UserSort>[] = [
  { sort: UserSort.User, label: "User", first: SortDirection.Asc },
  { sort: UserSort.Traces, label: "Traces", first: SortDirection.Desc },
  { sort: UserSort.Errors, label: "Errors", first: SortDirection.Desc },
  { sort: UserSort.Steps, label: "Steps", first: SortDirection.Desc },
  { sort: UserSort.Tokens, label: "Tokens", first: SortDirection.Desc },
  { sort: UserSort.FirstSeen, label: "First seen", first: SortDirection.Asc },
  { sort: UserSort.LastSeen, label: "Last seen", first: SortDirection.Desc },
];

// The server's default: most tokens first, until a call is priced and spend
// takes its place.
const DEFAULT_SORT = UserSort.Tokens;

/** The query the URL carries, every value the server would refuse dropped. */
export function parseUserQuery(searchParams: SearchParams): UserQuery {
  return compact({
    sort: oneOf(searchParams.sort, UserSort),
    dir: oneOf(searchParams.dir, SortDirection),
    ...parseWindow(searchParams),
  });
}

/** The list's path with `query` in its URL: `patch` changes some keys, and a
 *  key patched to `undefined` is dropped. */
export function usersHref(
  organization: string,
  project: string,
  query: UserQuery,
  patch: Partial<UserQuery> = {},
): string {
  return listHref(organization, project, "/users", query, patch);
}

/** The sort in force: the URL's, else the server's default. */
export function currentUserSort(query: UserQuery): { sort: UserSort; dir: SortDirection } {
  return sortInForce(USER_COLUMNS, DEFAULT_SORT, query);
}

/** What clicking a column heading asks for. */
export function nextUserSort(
  query: UserQuery,
  column: UserSort,
): { sort: UserSort; dir: SortDirection } {
  return sortAfterClick(USER_COLUMNS, DEFAULT_SORT, query, column);
}
