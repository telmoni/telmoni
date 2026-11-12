import { cache } from "react";
import { z } from "zod";

import { queryString } from "@/lib/list-query";
import { SessionSort, SortDirection, SpanStatus } from "@/lib/types/enums";
import { RowScoresSchema, ScoreSummarySchema, withScores } from "@/lib/types/traces";

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

// What a session's row and an end user's (`users.ts`) both hold: their
// traces in the window, summed by the server.
export const SummedSchema = z.object({
  /** The agents the traces ran, each once, in order. */
  agents: z.array(z.string()),
  traces: z.number().int(),
  failed_traces: z.number().int(),
  input_tokens: z.number(),
  output_tokens: z.number(),
  spend: z.number(),
  /** The traces' own time, summed: never the time between them. */
  duration_ms: z.number(),
  first_seen: z.string(),
  last_seen: z.string(),
  /** Whether the agent's own span of the trace that started last has arrived. */
  last_ended: z.boolean(),
  last_status: z.enum(SpanStatus),
  /** The traces' tags, each `key:value` once, in order. */
  tags: z.array(z.string()),
});
export type Summed = z.infer<typeof SummedSchema>;

// Sessions: a project's traces grouped by the id the application gave each
// (`session_id`, sent as `gen_ai.conversation.id`), as stored: hashed.
// Sign-in sessions are `active-sessions.ts`'s.
const SessionSchema = SummedSchema.extend({
  session_id: z.string(),
  /** The end user it was with, as stored; empty where none was sent. */
  user_id: z.string(),
  /** Its own scores by name, those given to the session and never its
   *  traces', where the list read them. */
  scores: z.array(ScoreSummarySchema).default([]),
});
export type Session = z.infer<typeof SessionSchema>;

// A page of the list, and how many sessions the filters admit on every page.
export const SessionListSchema = z
  .object({
    sessions: z.array(SessionSchema),
    days: z.number(),
    limit: z.number(),
    page: z.number(),
    total: z.number(),
    more: z.boolean(),
    sort: z.enum(SessionSort),
    dir: z.enum(SortDirection),
    /** Each session's own scores by name, keyed by its id: put on it. */
    scores: RowScoresSchema,
  })
  .transform(({ scores, ...list }) => ({
    ...list,
    sessions: withScores(list.sessions, scores, (session) => session.session_id),
  }));
export type SessionList = z.infer<typeof SessionListSchema>;

/** What narrows and orders the list, and which page of it shows, as the URL
 *  carries it (`lib/sessions.ts`). */
export type SessionQuery = ListQuery<SessionSort>;

export type SessionsAccess =
  | { kind: "ok"; list: SessionList }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** A page of the project's sessions in the window, as the server lists them. */
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

/** Each filter's values among the project's traces, and how many sessions
 *  hold a trace carrying each under every other filter. */
export const fetchSessionFacets = cache(
  async (projectId: string, query: SessionQuery): Promise<FacetsAccess> => {
    const read = await readLane(
      "fetchSessionFacets",
      projectId,
      `/sessions/facets${queryString(filtersOf(query))}`,
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
  return readDownload("sessionDownload", projectId, `/sessions/export${downloadQuery(query, format)}`);
}
