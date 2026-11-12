import { SortDirection, UserSort } from "@/lib/types/enums";

import { listHref } from "./list-query";
import { SummedColumn } from "./sessions";
import { listSpec, type ColumnSpec, type ListTable } from "./traces";

// The users list's URL is its state, as the trace list's is, built from the
// same parts as the sessions list (`sessions.ts`). A user here is an end
// user: the person a trace was for, by the id the application gave them —
// not a member of the project, which Members lists.

/** A column of the users list a person shows or hides, by the word their
 *  browser keeps it under (`lib/trace-prefs.ts`). */
export const UserColumn = {
  ...SummedColumn,
  Sessions: "sessions",
} as const;
export type UserColumn = typeof UserColumn[keyof typeof UserColumn];

/** The user's id, which every row shows, since it opens the user's page. */
export const USER_ID_COLUMN = {
  label: "User",
  hint: "The end user's id as stored, hashed under the project's own key, which opens the user's page. Always shown.",
} as const;

/** Every column but the user's id, in the table's order: what they used and
 *  how they stand first, then what it took. */
export const USER_COLUMNS: readonly ColumnSpec<UserColumn, UserSort>[] = [
  {
    key: UserColumn.Agents,
    label: "Agent",
    hint: "The agents whose traces were for the user.",
  },
  {
    key: UserColumn.LastSeen,
    label: "Last trace",
    sort: UserSort.LastSeen,
    first: SortDirection.Desc,
    hint: "When their last trace in the window ended, and how the one that started last stands.",
  },
  {
    key: UserColumn.Sessions,
    label: "Sessions",
    sort: UserSort.Sessions,
    first: SortDirection.Desc,
    numeric: true,
    hint: "How many sessions their traces named in the window. It opens those sessions.",
  },
  {
    key: UserColumn.Traces,
    label: "Traces",
    sort: UserSort.Traces,
    first: SortDirection.Desc,
    numeric: true,
    hint: "How many traces were for the user in the window.",
  },
  {
    key: UserColumn.FailedTraces,
    label: "Failed",
    sort: UserSort.FailedTraces,
    first: SortDirection.Desc,
    numeric: true,
    hint: "How many of their traces ended in error.",
  },
  {
    key: UserColumn.Duration,
    label: "Duration",
    sort: UserSort.Duration,
    first: SortDirection.Desc,
    numeric: true,
    hint: "How long their traces ran, summed: never the time between them.",
  },
  {
    key: UserColumn.Tokens,
    label: "Tokens",
    sort: UserSort.Tokens,
    first: SortDirection.Desc,
    numeric: true,
    hint: "Their traces' input and output tokens, summed.",
  },
  {
    key: UserColumn.Spend,
    label: "Spend",
    sort: UserSort.Spend,
    first: SortDirection.Desc,
    numeric: true,
    hint: "What their traces' model calls cost, each as the trace list prices it.",
  },
  {
    key: UserColumn.FirstSeen,
    label: "First trace",
    sort: UserSort.FirstSeen,
    first: SortDirection.Asc,
    hint: "When their first trace in the window started.",
  },
  {
    key: UserColumn.Tags,
    label: "Tags",
    hint: "Their traces' tags, each key:value once.",
  },
];

/** The columns the list shows until a person picks others: who was served,
 *  how much and how it went. */
export const DEFAULT_USER_COLUMNS: readonly UserColumn[] = [
  UserColumn.Agents,
  UserColumn.LastSeen,
  UserColumn.Sessions,
  UserColumn.Traces,
  UserColumn.FailedTraces,
  UserColumn.Tokens,
  UserColumn.Spend,
];

/** The users list's columns menu. */
export const USER_TABLE: ListTable<UserColumn, UserSort> = {
  id: USER_ID_COLUMN,
  columns: USER_COLUMNS,
  defaults: DEFAULT_USER_COLUMNS,
};

/** The users list: its table, menu, links and sorts; the server's default is
 *  the user last heard from first. */
export const USERS = listSpec<UserColumn, UserSort>({
  path: "/users",
  table: USER_TABLE,
  sorts: UserSort,
  defaultSort: UserSort.LastSeen,
  presets: [
    { id: "errors", label: "Errors", hint: "Only the users holding a trace that ended in error" },
    { id: "slow", label: "Slow", hint: "The users whose traces ran longest first" },
    { id: "tokens", label: "Tokens", hint: "The users that used the most tokens first" },
  ],
  presetSorts: { slow: UserSort.Duration, tokens: UserSort.Tokens },
});

export const parseUserQuery = USERS.parse;
export const usersHref = USERS.href;

/** An end user's own page over `days`, the page's own where none are named. */
export function userHref(organization: string, project: string, userId: string, days?: number): string {
  return listHref(organization, project, `/users/${encodeURIComponent(userId)}`, { days });
}
