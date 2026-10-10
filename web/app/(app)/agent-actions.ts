"use server";

import { z } from "zod";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { isProjectId } from "@/lib/connect";
import {
  agentStatus,
  deleteAgentConversation,
  listAgentConversations,
  openAgentConversation,
} from "@/lib/server/entities/agent";
import { getServerSession } from "@/lib/server/session";
import type {
  AgentConversation,
  AgentConversationSummary,
  AgentStatus,
} from "@/lib/types/agent";

// The agent panel's reads and its one delete. The turn itself is not here: it
// streams, and a Server Action answers once, so it is the Route Handler at
// `app/api/agent/turns`.

const EXPIRED = "Your session expired — sign in again.";
const SLOW_DOWN = "Too many requests — slow down a moment.";
const UNKNOWN_PROJECT = "Unknown project.";
const UNAVAILABLE = "The agent is unavailable right now — try again shortly.";

async function admit(action: string, limit: number, projectId: string): Promise<string | null> {
  const session = await getServerSession();
  if (!session) return EXPIRED;
  const limited = await rateLimit(sessionKey(session, action), { limit, windowMs: 60_000 });
  if (limited) return SLOW_DOWN;
  if (!isProjectId(projectId)) return UNKNOWN_PROJECT;
  return null;
}

export async function agentStatusAction(
  projectId: string,
): Promise<{ status: AgentStatus } | { error: string }> {
  const refused = await admit("agent:status", 30, projectId);
  if (refused) return { error: refused };
  return { status: await agentStatus(projectId) };
}

export async function listAgentConversationsAction(
  projectId: string,
): Promise<{ conversations: AgentConversationSummary[] } | { error: string }> {
  const refused = await admit("agent:list", 30, projectId);
  if (refused) return { error: refused };
  const conversations = await listAgentConversations(projectId);
  return conversations ? { conversations } : { error: UNAVAILABLE };
}

export async function openAgentConversationAction(
  projectId: string,
  id: string,
): Promise<{ conversation: AgentConversation } | { error: string }> {
  const refused = await admit("agent:open", 60, projectId);
  if (refused) return { error: refused };
  if (!z.guid().safeParse(id).success) return { error: "Unknown conversation." };
  const opened = await openAgentConversation(projectId, id);
  if (opened.kind === "ok") return { conversation: opened.conversation };
  if (opened.kind === "not-found") return { error: "That conversation no longer exists." };
  return { error: UNAVAILABLE };
}

export async function deleteAgentConversationAction(
  projectId: string,
  id: string,
): Promise<{ error: string | null }> {
  const refused = await admit("agent:delete", 30, projectId);
  if (refused) return { error: refused };
  if (!z.guid().safeParse(id).success) return { error: "Unknown conversation." };
  return (await deleteAgentConversation(projectId, id))
    ? { error: null }
    : { error: "Couldn't delete that conversation — try again shortly." };
}
