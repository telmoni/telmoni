import type {
  AgentConversation,
  AgentConversationSummary,
  AgentStatus,
} from "@/lib/types/agent";

// The agent panel's reads, from the agent's Route Handlers (`app/api/v1/agent`),
// made in the browser and so kept out of `lib/`, where a bare fetch is a
// server's. A refusal comes back as the sentence the panel shows: the
// handler's own, or — for a lapsed session the console's proxy turned away
// on `/api/*` before any handler ran — the one the handler would have given.

const EXPIRED = "Your session expired — sign in again.";
const UNREACHABLE = "The agent is unavailable right now — try again shortly.";

async function read<T extends object>(path: string): Promise<T | { error: string }> {
  try {
    const res = await fetch(path);
    const body = (await res.json().catch(() => null)) as (T & { error?: string }) | null;
    if (res.ok && body && !("error" in body)) return body;
    if (res.status === 401) return { error: EXPIRED };
    return { error: body?.error || UNREACHABLE };
  } catch {
    return { error: UNREACHABLE };
  }
}

const query = (projectId: string) => `project=${encodeURIComponent(projectId)}`;

export function readAgentStatus(projectId: string) {
  return read<{ status: AgentStatus }>(`/api/v1/agent/status?${query(projectId)}`);
}

export function readAgentConversations(projectId: string) {
  return read<{ conversations: AgentConversationSummary[] }>(
    `/api/v1/agent/conversations?${query(projectId)}`,
  );
}

export function readAgentConversation(projectId: string, id: string) {
  return read<{ conversation: AgentConversation }>(
    `/api/v1/agent/conversations/${encodeURIComponent(id)}?${query(projectId)}`,
  );
}
