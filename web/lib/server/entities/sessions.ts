import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { queryString } from "@/lib/list-query";
import { logger } from "@/lib/logger";
import { SessionSort, SortDirection } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";
import { TraceSchema } from "./traces";

// Sessions: a project's runs grouped by the id the application gave each
// session (`session_id`, sent as `gen_ai.conversation.id`). Sign-in sessions
// are `active-sessions.ts`'s. One session of the page is its runs in the
// window, summed by the server, with the end user it was with, as stored —
// hashed, unless the project keeps ids plain.
const SessionRowSchema = z.object({
  session_id: z.string(),
  user_id: z.string(),
  traces: z.number(),
  errors: z.number(),
  steps: z.number(),
  input_tokens: z.number(),
  output_tokens: z.number(),
  first_seen: z.string(),
  last_seen: z.string(),
});
export type SessionRow = z.infer<typeof SessionRowSchema>;

const SessionListSchema = z.object({
  sessions: z.array(SessionRowSchema),
  days: z.number(),
  limit: z.number(),
  more: z.boolean(),
  sort: z.enum(SessionSort),
  dir: z.enum(SortDirection),
});
export type SessionList = z.infer<typeof SessionListSchema>;

// A session's own page: its traces in the order they ran, each summed as the
// trace list sums it — a replay in metadata, never a message.
const SessionDetailSchema = z.object({
  session_id: z.string(),
  traces: z.array(TraceSchema),
  days: z.number(),
  limit: z.number(),
  more: z.boolean(),
});
export type SessionDetail = z.infer<typeof SessionDetailSchema>;

/** What orders and bounds the page, as the URL carries it: a key left out
 *  asks for the default. The server checks every value; the page drops what
 *  it cannot read before asking (`lib/sessions.ts`). */
export type SessionQuery = {
  sort?: SessionSort;
  dir?: SortDirection;
  days?: number;
  limit?: number;
};

export type SessionsAccess =
  | { kind: "ok"; list: SessionList }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type SessionAccess =
  | { kind: "ok"; session: SessionDetail }
  /** No run of this project's in the window belonged to that session. */
  | { kind: "missing" }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

function fetchFailed(fetcher: string, err: unknown): void {
  logger.warn(
    { fetcher, error: err instanceof Error ? err.message : String(err) },
    "entities: fetch failed",
  );
}

/** The project's sessions in the window, as the server lists them. */
export const fetchSessions = cache(
  async (projectId: string, query: SessionQuery): Promise<SessionsAccess> => {
    if (!projectId) return { kind: "unavailable" };
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/telemetry/sessions${queryString(query)}`,
        { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      );
      if (res.status === 403) return { kind: "forbidden" };
      if (!res.ok) {
        logger.warn(
          { fetcher: "fetchSessions", status: res.status },
          "entities: upstream refused",
        );
        return { kind: "unavailable" };
      }
      const parsed = SessionListSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchSessions", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return { kind: "unavailable" };
      }
      return { kind: "ok", list: parsed.data };
    } catch (err) {
      fetchFailed("fetchSessions", err);
      return { kind: "unavailable" };
    }
  },
);

/** One session's traces in the window, by its id, which the path carries
 *  URL-encoded since it is the application's own text. An id the server
 *  refuses — empty, or past its length — is read as missing. */
export const fetchSession = cache(
  async (
    projectId: string,
    sessionId: string,
    query: Pick<SessionQuery, "days" | "limit">,
  ): Promise<SessionAccess> => {
    if (!projectId) return { kind: "unavailable" };
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/telemetry/sessions/${encodeURIComponent(sessionId)}${queryString(query)}`,
        { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      );
      if (res.status === 403) return { kind: "forbidden" };
      if (res.status === 404 || res.status === 400) return { kind: "missing" };
      if (!res.ok) {
        logger.warn(
          { fetcher: "fetchSession", status: res.status },
          "entities: upstream refused",
        );
        return { kind: "unavailable" };
      }
      const parsed = SessionDetailSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchSession", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return { kind: "unavailable" };
      }
      return { kind: "ok", session: parsed.data };
    } catch (err) {
      fetchFailed("fetchSession", err);
      return { kind: "unavailable" };
    }
  },
);
