import { cache } from "react";
import { z } from "zod";

import { queryString } from "@/lib/list-query";
import { SortDirection, UserSort } from "@/lib/types/enums";

import { SummedSchema } from "./sessions";
import {
  FacetsSchema,
  downloadQuery,
  filtersOf,
  readDownload,
  readLane,
  type DownloadAccess,
  type DownloadFormat,
  type FacetsAccess,
  type ListQuery,
} from "./traces";

// End users: a project's traces grouped by the id the application gave the
// person each was for (`user_id`), as stored: hashed. The console's own
// people are members (`member.ts`). One row is an end user's traces in the
// window, summed by the server.
const UserSchema = SummedSchema.extend({
  user_id: z.string(),
  /** The sessions their traces named in the window, each once. */
  sessions: z.number().int(),
});
export type EndUser = z.infer<typeof UserSchema>;

// A page of the list, and how many users the filters admit on every page.
export const UserListSchema = z.object({
  users: z.array(UserSchema),
  days: z.number(),
  limit: z.number(),
  page: z.number(),
  total: z.number(),
  more: z.boolean(),
  sort: z.enum(UserSort),
  dir: z.enum(SortDirection),
});
export type UserList = z.infer<typeof UserListSchema>;

/** What narrows and orders the list, and which page of it shows, as the URL
 *  carries it (`lib/users.ts`). */
export type UserQuery = ListQuery<UserSort>;

export type UsersAccess =
  | { kind: "ok"; list: UserList }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** A page of the project's end users in the window, as the server lists them. */
export const fetchUsers = cache(
  async (projectId: string, query: UserQuery): Promise<UsersAccess> => {
    const read = await readLane("fetchUsers", projectId, `/users${queryString(query)}`, UserListSchema);
    return read.kind === "ok" ? { kind: "ok", list: read.value } : read;
  },
);

/** Each filter's values among the project's traces, and how many users hold
 *  a trace carrying each under every other filter. */
export const fetchUserFacets = cache(
  async (projectId: string, query: UserQuery): Promise<FacetsAccess> => {
    const read = await readLane(
      "fetchUserFacets",
      projectId,
      `/users/facets${queryString(filtersOf(query))}`,
      FacetsSchema,
    );
    return read.kind === "ok" ? { kind: "ok", facets: read.value } : read;
  },
);

/** The list as a file: every user the filters admit, sorted as the page is,
 *  up to the server's cap. */
export async function userDownload(
  projectId: string,
  query: UserQuery,
  format: DownloadFormat,
): Promise<DownloadAccess> {
  return readDownload("userDownload", projectId, `/users/export${downloadQuery(query, format)}`);
}
