import { z } from "zod";

import { CitationData } from "@/lib/agent/stream";
import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import {
  type AgentConversation,
  type AgentConversationSummary,
  type AgentStatus,
} from "@/lib/types/agent";
import { AgentMessageRole } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";
import { fetchProjectAnywhere } from "./projects";
import { readFailed, readJson } from "./read";

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
