import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { queryString } from "@/lib/list-query";
import { logger } from "@/lib/logger";
import { SortDirection, UserSort } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";

// One user of the page: a user id's runs in the window, summed by the server,
// which reads the project's spans and no other's. `user_id` is as stored —
// hashed, unless the project keeps ids plain — so the page names nobody the
// project did not choose to name.
const UserRowSchema = z.object({
  user_id: z.string(),
  traces: z.number(),
  errors: z.number(),
  steps: z.number(),
  input_tokens: z.number(),
  output_tokens: z.number(),
  first_seen: z.string(),
  last_seen: z.string(),
});
export type UserRow = z.infer<typeof UserRowSchema>;

const UserListSchema = z.object({
  users: z.array(UserRowSchema),
  days: z.number(),
  limit: z.number(),
  more: z.boolean(),
  sort: z.enum(UserSort),
  dir: z.enum(SortDirection),
});
export type UserList = z.infer<typeof UserListSchema>;

/** What orders and bounds the page, as the URL carries it: a key left out
 *  asks for the default. The server checks every value; the page drops what
 *  it cannot read before asking (`lib/users.ts`). */
export type UserQuery = {
  sort?: UserSort;
  dir?: SortDirection;
  days?: number;
  limit?: number;
};

export type UsersAccess =
  | { kind: "ok"; list: UserList }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** The project's users in the window, as the server lists them. */
export const fetchUsers = cache(
  async (projectId: string, query: UserQuery): Promise<UsersAccess> => {
    if (!projectId) return { kind: "unavailable" };
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/v1/telemetry/users${queryString(query)}`,
        { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      );
      if (res.status === 403) return { kind: "forbidden" };
      if (!res.ok) {
        logger.warn({ fetcher: "fetchUsers", status: res.status }, "entities: upstream refused");
        return { kind: "unavailable" };
      }
      const parsed = UserListSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchUsers", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return { kind: "unavailable" };
      }
      return { kind: "ok", list: parsed.data };
    } catch (err) {
      logger.warn(
        { fetcher: "fetchUsers", error: err instanceof Error ? err.message : String(err) },
        "entities: fetch failed",
      );
      return { kind: "unavailable" };
    }
  },
);
