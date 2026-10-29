import { cache } from "react";
import { z } from "zod";

import { queryString } from "@/lib/list-query";
import { SessionSort, SortDirection } from "@/lib/types/enums";
import { SessionDetailSchema, type SessionDetail } from "@/lib/types/sessions";

import {
  FacetsSchema,
  MISSING,
  filtersOf,
  readDownload,
  readLane,
  type DownloadAccess,
  type DownloadFormat,
  type FacetsAccess,
  type ListQuery,
} from "./traces";

// Sessions: a project's runs grouped by the id the application gave each
// session (`session_id`, sent as `gen_ai.conversation.id`). Sign-in sessions
// are `active-sessions.ts`'s. One session of the page is its runs in the
// window, summed by the server, with the end user it was with, as stored,
// hashed.
const SessionRowSchema = z.object({
  session_id: z.string(),
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
export type SessionRow = z.infer<typeof SessionRowSchema>;

// A page of the list, and how many sessions the filters admit on every page.
export const SessionListSchema = z.object({
  sessions: z.array(SessionRowSchema),
  days: z.number(),
  limit: z.number(),
  page: z.number(),
  total: z.number(),
  more: z.boolean(),
  sort: z.enum(SessionSort),
  dir: z.enum(SortDirection),
});
export type SessionList = z.infer<typeof SessionListSchema>;

export {
  PinnedSpanSchema,
  SessionDetailSchema,
  SessionTraceSchema,
  type PinnedSpan,
  type SessionDetail,
  type SessionTrace,
} from "@/lib/types/sessions";

/** What narrows and orders the list, and which page of it shows, as the URL
 *  carries it (`lib/sessions.ts`). */
export type SessionQuery = ListQuery<SessionSort>;

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

/** The project's sessions in the window, as the server lists them. */
export const fetchSessions = cache(
  async (projectId: string, query: SessionQuery): Promise<SessionsAccess> => {
    const read = await readLane(
      "fetchSessions",
      projectId,
      `/sessions${queryString(query)}`,
      SessionListSchema,
    );
    return read.kind === "ok" ? { kind: "ok", list: read.value } : read;
  },
);

/** Each filter's values among the project's runs, and how many sessions hold
 *  a run carrying each under every other filter. */
export const fetchSessionFacets = cache(
  async (projectId: string, query: SessionQuery): Promise<FacetsAccess> => {
    const read = await readLane(
      "fetchSessionFacets",
      projectId,
      `/session-facets${queryString(filtersOf(query))}`,
      FacetsSchema,
    );
    return read.kind === "ok" ? { kind: "ok", facets: read.value } : read;
  },
);

/** The list as a file: every session the filters admit, sorted as the page
 *  is, up to the server's cap. */
export async function sessionDownload(
  projectId: string,
  query: SessionQuery,
  format: DownloadFormat,
): Promise<DownloadAccess> {
  const lane = queryString({ ...filtersOf(query), sort: query.sort, dir: query.dir, format });
  return readDownload("sessionDownload", projectId, `/session-export${lane}`);
}

/** Which spans a session's traces are read with under them: the names the
 *  browser's own layout pins, or the layout the server resolves itself — a
 *  saved one by its id, falling back to the project's, or the project's own
 *  as `"project"`. */
export type SessionLayoutRead = { pinned: readonly string[] } | { view: string };

/** One session's traces in the window, by its id, which the path carries
 *  URL-encoded since it is the application's own text, each with the spans
 *  `layout` pins under it. An id the server refuses — empty, or past its
 *  length — is read as missing. */
export const fetchSession = cache(
  async (
    projectId: string,
    sessionId: string,
    query: Pick<SessionQuery, "days" | "limit">,
    layout: SessionLayoutRead,
  ): Promise<SessionAccess> => {
    const read = await readLane(
      "fetchSession",
      projectId,
      `/sessions/${encodeURIComponent(sessionId)}${queryString({ ...query, ...layout })}`,
      SessionDetailSchema,
      MISSING,
    );
    return read.kind === "ok" ? { kind: "ok", session: read.value } : read;
  },
);
