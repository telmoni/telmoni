import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { CitationData, describeRefusal } from "@/lib/agent/stream";
import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import {
  type AgentConversation,
  type AgentConversationSummary,
  type AgentStatus,
  TRACE_QUESTION_TIMEOUT_MS,
} from "@/lib/types/agent";
import { AgentMessageRole } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";
import { fetchProjectAnywhere } from "./projects";
import { readFailed, readJson } from "./read";
import { ViewQuerySchema } from "./traces";

export const StatusSchema = z.object({ enabled: z.boolean() });

export const ConversationListSchema = z.object({
  conversations: z.array(
    z.object({
      id: z.string(),
      title: z.string(),
      project_id: z.string(),
      created_at: z.string(),
      updated_at: z.string(),
    }),
  ),
});

export const ConversationSchema = z.object({
  id: z.string(),
  title: z.string(),
  project_id: z.string(),
  created_at: z.string(),
  messages: z.array(
    z.object({
      id: z.string(),
      role: z.enum(AgentMessageRole),
      content: z.string(),
      citations: z.array(CitationData),
      created_at: z.string(),
    }),
  ),
});

// A sentence asked of the trace list: the saved view's query it opens on, or
// `null` where the sentence could not be read as one, what the filters could
// not say, and the values the window holds no trace of.
export const TraceFiltersSchema = z.object({
  query: ViewQuerySchema.nullable(),
  unmet: z.string().nullable(),
  dropped: z.array(z.object({ filter: z.string(), value: z.string() })),
});
export type TraceFiltersAnswer = z.infer<typeof TraceFiltersSchema>;

// The headers for an agent lane, or `null` when the person or the project
// cannot be resolved. The project is checked against the person's own list
// first so a stale id costs no round trip; the server decides again anyway.
// Its own organization is the one named: the turns route's path names none.
export async function agentHeaders(projectId: string): Promise<Record<string, string> | null> {
  const [ctx, project] = await Promise.all([identityContext(), fetchProjectAnywhere(projectId)]);
  if (!ctx || !project) return null;
  return projectHeaders({ ...ctx, organizationId: project.organizationId }, projectId);
}

export async function agentStatus(projectId: string): Promise<AgentStatus> {
  const headers = await agentHeaders(projectId);
  if (!headers) return "unavailable";
  const read = await readJson(
    "agentStatus",
    `${env.SERVER_URL}/internal/v1/agent/status`,
    { headers },
    StatusSchema,
  );
  if (read.kind !== "ok") return "unavailable";
  return read.value.enabled ? "enabled" : "disabled";
}

export async function listAgentConversations(
  projectId: string,
): Promise<AgentConversationSummary[] | null> {
  const headers = await agentHeaders(projectId);
  if (!headers) return null;
  const read = await readJson(
    "listAgentConversations",
    `${env.SERVER_URL}/internal/v1/agent/conversations`,
    { headers },
    ConversationListSchema,
  );
  if (read.kind !== "ok") return null;
  return read.value.conversations.map((c) => ({
    id: c.id,
    title: c.title,
    createdAt: c.created_at,
    updatedAt: c.updated_at,
  }));
}

export type OpenedConversation =
  | { kind: "ok"; conversation: AgentConversation }
  | { kind: "missing" }
  | { kind: "unavailable" };

export async function openAgentConversation(
  projectId: string,
  id: string,
): Promise<OpenedConversation> {
  const headers = await agentHeaders(projectId);
  if (!headers) return { kind: "unavailable" };
  const read = await readJson(
    "openAgentConversation",
    `${env.SERVER_URL}/internal/v1/agent/conversations/${encodeURIComponent(id)}`,
    { headers },
    ConversationSchema,
    { 404: "missing" },
  );
  if (read.kind === "missing") return { kind: "missing" };
  // The project was admitted before the read, so a refusal is the agent's outage.
  if (read.kind !== "ok") return { kind: "unavailable" };
  return {
    kind: "ok",
    conversation: {
      id: read.value.id,
      title: read.value.title,
      messages: read.value.messages.map((m) => ({
        id: m.id,
        role: m.role,
        content: m.content,
        citations: m.citations,
      })),
    },
  };
}

export async function deleteAgentConversation(projectId: string, id: string): Promise<boolean> {
  const headers = await agentHeaders(projectId);
  if (!headers) return false;
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/v1/agent/conversations/${encodeURIComponent(id)}`,
      { method: "DELETE", headers },
    );
    // Already gone is what the person asked for.
    if (res.ok || res.status === 404) return true;
    logger.warn(
      { fetcher: "deleteAgentConversation", status: res.status },
      "entities: upstream refused",
    );
    return false;
  } catch (err) {
    readFailed("deleteAgentConversation", err);
    return false;
  }
}

/** The trace list asked `sentence` over its last `days`: one of the plan's
 *  agent questions, refused in the panel's words. */
export async function askTraceFilters(
  projectId: string,
  sentence: string,
  days: number,
): Promise<ActionResult<{ answer: TraceFiltersAnswer }>> {
  const headers = await agentHeaders(projectId);
  if (!headers) return refused("Unknown project.");
  let res: Response;
  try {
    res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/v1/agent/trace-filters`,
      {
        method: "POST",
        headers: { ...headers, "content-type": "application/json" },
        body: JSON.stringify({ sentence, days }),
      },
      TRACE_QUESTION_TIMEOUT_MS,
    );
  } catch (err) {
    readFailed("askTraceFilters", err);
    return refused("The agent couldn't answer — try again shortly.");
  }
  const body: unknown = await res.json().catch(() => null);
  // The panel's not-found is a conversation's; here it is the project.
  if (res.status === 404) return refused("This project is no longer there.");
  if (!res.ok) {
    const { title, detail } = describeRefusal(res.status, body, res.headers.get("retry-after"));
    // The panel's own titles are sentences; a server's is terse words, and
    // its detail — the plan's refusal, say — the sentence.
    if (!detail) return refused(title);
    return refused(title.endsWith(".") ? `${title} ${detail}` : detail);
  }
  const answer = TraceFiltersSchema.safeParse(body);
  if (!answer.success) {
    logger.warn({ fetcher: "askTraceFilters" }, "entities: upstream answered a shape it should not");
    return refused("The agent couldn't answer — try again shortly.");
  }
  return { ok: true, answer: answer.data };
}
