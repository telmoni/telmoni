import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { queryString } from "@/lib/list-query";
import { logger } from "@/lib/logger";
import { ConversationSort, SortDirection } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";
import { TraceSchema } from "./traces";

// Conversations: a project's runs grouped by the id the application gave each
// conversation (`gen_ai.conversation.id`), which other tools call sessions or
// threads. Not `sessions.ts`, which reads sign-in sessions. One conversation
// of the page is its runs in the window, summed by the server, with the end
// user it was with, as stored — hashed, unless the project keeps ids plain.
const ConversationRowSchema = z.object({
  conversation_id: z.string(),
  user_id: z.string(),
  traces: z.number(),
  errors: z.number(),
  steps: z.number(),
  input_tokens: z.number(),
  output_tokens: z.number(),
  first_seen: z.string(),
  last_seen: z.string(),
});
export type ConversationRow = z.infer<typeof ConversationRowSchema>;

const ConversationListSchema = z.object({
  conversations: z.array(ConversationRowSchema),
  days: z.number(),
  limit: z.number(),
  more: z.boolean(),
  sort: z.enum(ConversationSort),
  dir: z.enum(SortDirection),
});
export type ConversationList = z.infer<typeof ConversationListSchema>;

// A conversation's own page: its traces in the order they ran, each summed as
// the trace list sums it — a replay in metadata, never a message.
const ConversationDetailSchema = z.object({
  conversation_id: z.string(),
  traces: z.array(TraceSchema),
  days: z.number(),
  limit: z.number(),
  more: z.boolean(),
});
export type ConversationDetail = z.infer<typeof ConversationDetailSchema>;

/** What orders and bounds the page, as the URL carries it: a key left out
 *  asks for the default. The server checks every value; the page drops what
 *  it cannot read before asking (`lib/conversations.ts`). */
export type ConversationQuery = {
  sort?: ConversationSort;
  dir?: SortDirection;
  days?: number;
  limit?: number;
};

export type ConversationsAccess =
  | { kind: "ok"; list: ConversationList }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type ConversationAccess =
  | { kind: "ok"; conversation: ConversationDetail }
  /** No run of this project's in the window belonged to that conversation. */
  | { kind: "missing" }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

function fetchFailed(fetcher: string, err: unknown): void {
  logger.warn(
    { fetcher, error: err instanceof Error ? err.message : String(err) },
    "entities: fetch failed",
  );
}

/** The project's conversations in the window, as the server lists them. */
export const fetchConversations = cache(
  async (projectId: string, query: ConversationQuery): Promise<ConversationsAccess> => {
    if (!projectId) return { kind: "unavailable" };
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/telemetry/conversations${queryString(query)}`,
        { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      );
      if (res.status === 403) return { kind: "forbidden" };
      if (!res.ok) {
        logger.warn(
          { fetcher: "fetchConversations", status: res.status },
          "entities: upstream refused",
        );
        return { kind: "unavailable" };
      }
      const parsed = ConversationListSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchConversations", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return { kind: "unavailable" };
      }
      return { kind: "ok", list: parsed.data };
    } catch (err) {
      fetchFailed("fetchConversations", err);
      return { kind: "unavailable" };
    }
  },
);

/** One conversation's traces in the window, by its id, which the path carries
 *  URL-encoded since it is the application's own text. An id the server
 *  refuses — empty, or past its length — is read as missing. */
export const fetchConversation = cache(
  async (
    projectId: string,
    conversationId: string,
    query: Pick<ConversationQuery, "days" | "limit">,
  ): Promise<ConversationAccess> => {
    if (!projectId) return { kind: "unavailable" };
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/telemetry/conversations/${encodeURIComponent(conversationId)}${queryString(query)}`,
        { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      );
      if (res.status === 403) return { kind: "forbidden" };
      if (res.status === 404 || res.status === 400) return { kind: "missing" };
      if (!res.ok) {
        logger.warn(
          { fetcher: "fetchConversation", status: res.status },
          "entities: upstream refused",
        );
        return { kind: "unavailable" };
      }
      const parsed = ConversationDetailSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchConversation", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return { kind: "unavailable" };
      }
      return { kind: "ok", conversation: parsed.data };
    } catch (err) {
      fetchFailed("fetchConversation", err);
      return { kind: "unavailable" };
    }
  },
);
