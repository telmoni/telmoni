import type { ConversationQuery } from "@/lib/server/entities/conversations";
import { ConversationSort, SortDirection } from "@/lib/types/enums";

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

// The conversations list's URL is its state, as the trace list's is: the
// sort and the window, read with every value the server would refuse
// dropped, and written with one thing changed. A conversation is what other
// tools call a session or a thread — not a sign-in session, which
// `lib/server/entities/sessions.ts` reads.

/** The columns the list sorts by, in the table's order, each with the way it
 *  sorts when first clicked: ids and first seen the alphabet's and the
 *  calendar's way, the most and the latest first on the others. */
export const CONVERSATION_COLUMNS: readonly Column<ConversationSort>[] = [
  { sort: ConversationSort.Conversation, label: "Conversation", first: SortDirection.Asc },
  { sort: ConversationSort.User, label: "User", first: SortDirection.Asc },
  { sort: ConversationSort.Traces, label: "Traces", first: SortDirection.Desc },
  { sort: ConversationSort.Errors, label: "Errors", first: SortDirection.Desc },
  { sort: ConversationSort.Steps, label: "Steps", first: SortDirection.Desc },
  { sort: ConversationSort.Tokens, label: "Tokens", first: SortDirection.Desc },
  { sort: ConversationSort.FirstSeen, label: "First seen", first: SortDirection.Asc },
  { sort: ConversationSort.LastSeen, label: "Last seen", first: SortDirection.Desc },
];

// The server's default: the conversation last heard from first.
const DEFAULT_SORT = ConversationSort.LastSeen;

/** The query the URL carries, every value the server would refuse dropped. */
export function parseConversationQuery(searchParams: SearchParams): ConversationQuery {
  return compact({
    sort: oneOf(searchParams.sort, ConversationSort),
    dir: oneOf(searchParams.dir, SortDirection),
    ...parseWindow(searchParams),
  });
}

/** The list's path with `query` in its URL: `patch` changes some keys, and a
 *  key patched to `undefined` is dropped. */
export function conversationsHref(
  organization: string,
  project: string,
  query: ConversationQuery,
  patch: Partial<ConversationQuery> = {},
): string {
  return listHref(organization, project, "/conversations", query, patch);
}

/** A conversation's own page, by its id, which the path carries URL-encoded:
 *  it is up to 200 characters of the application's choosing, and Next hands
 *  the page the segment decoded again. */
export function conversationHref(
  organization: string,
  project: string,
  conversationId: string,
  query: WindowQuery = {},
): string {
  return listHref(
    organization,
    project,
    `/conversations/${encodeURIComponent(conversationId)}`,
    query,
  );
}

/** The sort in force: the URL's, else the server's default. */
export function currentConversationSort(query: ConversationQuery): {
  sort: ConversationSort;
  dir: SortDirection;
} {
  return sortInForce(CONVERSATION_COLUMNS, DEFAULT_SORT, query);
}

/** What clicking a column heading asks for. */
export function nextConversationSort(
  query: ConversationQuery,
  column: ConversationSort,
): { sort: ConversationSort; dir: SortDirection } {
  return sortAfterClick(CONVERSATION_COLUMNS, DEFAULT_SORT, query, column);
}
