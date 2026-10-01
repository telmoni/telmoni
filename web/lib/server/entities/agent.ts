import { z } from "zod";

import { CitationData } from "@/lib/agent/stream";
import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import {
  AGENT_ROLES,
  type AgentConversation,
  type AgentConversationSummary,
  type AgentStatus,
} from "@/lib/types/agent";

import { identityContext, projectHeaders } from "./identity-context";
import { fetchProject } from "./projects";

// How long one turn may stream. `fetchWithTimeout` bounds the whole exchange,
// body included, so this is the ceiling on a reply, not on its first byte: a
// turn that runs several tools before it writes is still inside it. The server
// ends a turn itself at `TURN_BUDGET` (100 s, `crates/agent/src/turn.rs`) and
// saves what it wrote, so this cut-off is only ever the backstop; keep it the
// longer of the two.
export const AGENT_STREAM_TIMEOUT_MS = 120_000;

const StatusSchema = z.object({ enabled: z.boolean() });

const ConversationListSchema = z.object({
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

const ConversationSchema = z.object({
  id: z.string(),
  title: z.string(),
  project_id: z.string(),
  created_at: z.string(),
  messages: z.array(
    z.object({
      id: z.string(),
      role: z.enum(AGENT_ROLES),
      content: z.string(),
      citations: z.array(CitationData),
      created_at: z.string(),
    }),
  ),
});

// The headers for an agent lane, or `null` when the person or the project
// cannot be resolved. The project is checked against the person's own list
// first so a stale id costs no round trip; the server decides again anyway.
export async function agentHeaders(projectId: string): Promise<Record<string, string> | null> {
  const [ctx, project] = await Promise.all([identityContext(), fetchProject(projectId)]);
  if (!ctx || !project) return null;
  return projectHeaders(ctx, projectId);
}

export async function agentStatus(projectId: string): Promise<AgentStatus> {
  const headers = await agentHeaders(projectId);
  if (!headers) return "unavailable";
  try {
    const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/agent/status`, { headers });
    if (!res.ok) {
      logger.warn({ fetcher: "agentStatus", status: res.status }, "entities: upstream refused");
      return "unavailable";
    }
    const parsed = StatusSchema.safeParse(await res.json());
    if (!parsed.success) {
      logger.warn(
        { fetcher: "agentStatus", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return "unavailable";
    }
    return parsed.data.enabled ? "enabled" : "disabled";
  } catch {
    logger.warn({ fetcher: "agentStatus" }, "entities: upstream error");
    return "unavailable";
  }
}

export async function listAgentConversations(
  projectId: string,
): Promise<AgentConversationSummary[] | null> {
  const headers = await agentHeaders(projectId);
  if (!headers) return null;
  try {
    const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/agent/conversations`, {
      headers,
    });
    if (!res.ok) {
      logger.warn(
        { fetcher: "listAgentConversations", status: res.status },
        "entities: upstream refused",
      );
      return null;
    }
    const parsed = ConversationListSchema.safeParse(await res.json());
    if (!parsed.success) {
      logger.warn(
        { fetcher: "listAgentConversations", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return null;
    }
    return parsed.data.conversations.map((c) => ({
      id: c.id,
      title: c.title,
      createdAt: c.created_at,
      updatedAt: c.updated_at,
    }));
  } catch {
    logger.warn({ fetcher: "listAgentConversations" }, "entities: upstream error");
    return null;
  }
}

export type OpenedConversation =
  | { kind: "ok"; conversation: AgentConversation }
  | { kind: "not-found" }
  | { kind: "unavailable" };

export async function openAgentConversation(
  projectId: string,
  id: string,
): Promise<OpenedConversation> {
  const headers = await agentHeaders(projectId);
  if (!headers) return { kind: "unavailable" };
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/agent/conversations/${encodeURIComponent(id)}`,
      { headers },
    );
    if (res.status === 404) return { kind: "not-found" };
    if (!res.ok) {
      logger.warn(
        { fetcher: "openAgentConversation", status: res.status },
        "entities: upstream refused",
      );
      return { kind: "unavailable" };
    }
    const parsed = ConversationSchema.safeParse(await res.json());
    if (!parsed.success) {
      logger.warn(
        { fetcher: "openAgentConversation", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return { kind: "unavailable" };
    }
    return {
      kind: "ok",
      conversation: {
        id: parsed.data.id,
        title: parsed.data.title,
        messages: parsed.data.messages.map((m) => ({
          id: m.id,
          role: m.role,
          content: m.content,
          citations: m.citations,
        })),
      },
    };
  } catch {
    logger.warn({ fetcher: "openAgentConversation" }, "entities: upstream error");
    return { kind: "unavailable" };
  }
}

export async function deleteAgentConversation(projectId: string, id: string): Promise<boolean> {
  const headers = await agentHeaders(projectId);
  if (!headers) return false;
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/agent/conversations/${encodeURIComponent(id)}`,
      { method: "DELETE", headers },
    );
    // Already gone is what the person asked for.
    if (res.ok || res.status === 404) return true;
    logger.warn(
      { fetcher: "deleteAgentConversation", status: res.status },
      "entities: upstream refused",
    );
    return false;
  } catch {
    logger.warn({ fetcher: "deleteAgentConversation" }, "entities: upstream error");
    return false;
  }
}
