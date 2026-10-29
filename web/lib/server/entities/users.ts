import { cache } from "react";
import { z } from "zod";

import { queryString } from "@/lib/list-query";
import { SortDirection, UserSort } from "@/lib/types/enums";

import {
  FacetsSchema,
  filtersOf,
  readDownload,
  readLane,
  type DownloadAccess,
  type DownloadFormat,
  type FacetsAccess,
  type ListQuery,
} from "./traces";

// One user of the page: a user id's runs in the window, summed by the server,
// which reads the project's spans and no other's. `user_id` is as stored,
// hashed, so the page names nobody.
const UserRowSchema = z.object({
  user_id: z.string(),
  traces: z.number(),
  errors: z.number(),
  steps: z.number(),
  input_tokens: z.number(),
  output_tokens: z.number(),
  duration_ms: z.number(),
  first_seen: z.string(),
  last_seen: z.string(),
});
export type UserRow = z.infer<typeof UserRowSchema>;

// A page of the list, and how many users the filters admit on every page.
export const UserListSchema = z.object({
  users: z.array(UserRowSchema),
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

/** The project's users in the window, as the server lists them. */
export const fetchUsers = cache(
  async (projectId: string, query: UserQuery): Promise<UsersAccess> => {
    const read = await readLane("fetchUsers", projectId, `/users${queryString(query)}`, UserListSchema);
    return read.kind === "ok" ? { kind: "ok", list: read.value } : read;
  },
);

/** Each filter's values among the project's runs, and how many users have a
 *  run carrying each under every other filter. */
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
  const lane = queryString({ ...filtersOf(query), sort: query.sort, dir: query.dir, format });
  return readDownload("userDownload", projectId, `/users/export${lane}`);
}
