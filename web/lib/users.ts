import { SortDirection, UserSort } from "@/lib/types/enums";

import { listSpec, type ColumnSpec, type ListTable } from "./traces";

// The users list's URL is its state, as the trace list's is: its filters,
// the sort, the window and the page, read with every value the server would
// refuse dropped, and written with one thing changed.

/** A column of the users table a person may show or hide. */
export type UserColumn =
  | "traces"
  | "errors"
  | "steps"
  | "input_tokens"
  | "output_tokens"
  | "tokens"
  | "first_seen"
  | "last_seen";

/** The user's id, which every row shows, since it opens their traces. */
export const USER_ID_COLUMN = {
  label: "User",
  hint: "The end user's id, as stored: hashed under the project's own key. Opens their traces. Always shown.",
} as const;

/** Every column but the user's id, in the table's order, each with the way
 *  it sorts when first clicked: first seen the calendar's way, the most and
 *  the latest first on the others. */
export const USER_COLUMNS: readonly ColumnSpec<UserColumn, UserSort>[] = [
  {
    key: "traces",
    label: "Traces",
    sort: UserSort.Traces,
    first: SortDirection.Desc,
    numeric: true,
    group: "counts",
    hint: "How many runs were for them in the window.",
  },
  {
    key: "errors",
    label: "Errors",
    sort: UserSort.Errors,
    first: SortDirection.Desc,
    numeric: true,
    group: "counts",
    hint: "How many of their runs had a span that failed.",
  },
  {
    key: "steps",
    label: "Steps",
    sort: UserSort.Steps,
    first: SortDirection.Desc,
    numeric: true,
    group: "counts",
  },
  {
    key: "input_tokens",
    label: "Input tokens",
    sort: UserSort.InputTokens,
    first: SortDirection.Desc,
    numeric: true,
    group: "tokens",
  },
  {
    key: "output_tokens",
    label: "Output tokens",
    sort: UserSort.OutputTokens,
    first: SortDirection.Desc,
    numeric: true,
    group: "tokens",
  },
  {
    key: "tokens",
    label: "Tokens",
    sort: UserSort.Tokens,
    first: SortDirection.Desc,
    numeric: true,
    group: "tokens",
    hint: "Input and output tokens, summed.",
  },
  {
    key: "first_seen",
    label: "First seen",
    sort: UserSort.FirstSeen,
    first: SortDirection.Asc,
    hint: "When their first run in the window started.",
  },
  {
    key: "last_seen",
    label: "Last seen",
    sort: UserSort.LastSeen,
    first: SortDirection.Desc,
    hint: "When their last run in the window ended.",
  },
];

/** The columns the table shows until a person picks others: what it showed
 *  before it offered the rest. */
export const DEFAULT_USER_COLUMNS: readonly UserColumn[] = [
  "traces",
  "errors",
  "steps",
  "tokens",
  "first_seen",
  "last_seen",
];

/** The users list's columns menu. */
export const USER_TABLE: ListTable<UserColumn, UserSort> = {
  id: USER_ID_COLUMN,
  columns: USER_COLUMNS,
  defaults: DEFAULT_USER_COLUMNS,
};

/** The users list: its table, menu, links and sorts, the id's among them;
 *  the server's default is most tokens first, until a call is priced and
 *  spend takes its place. */
export const USERS = listSpec({
  path: "/users",
  table: USER_TABLE,
  sorts: UserSort,
  idSort: { sort: UserSort.User, label: USER_ID_COLUMN.label, first: SortDirection.Asc },
  defaultSort: UserSort.Tokens,
});

export const USER_COLUMN_MENU = USERS.menu;
export const orderedUserColumns = USERS.ordered;
export const parseUserQuery = USERS.parse;
export const usersHref = USERS.href;
export const currentUserSort = USERS.current;
export const nextUserSort = USERS.next;
