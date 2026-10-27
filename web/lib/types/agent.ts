// The agent's conversation as the console holds it, camelCased from the
// server's wire shape at the entity (`lib/server/entities/agent.ts`) and at
// the stream parser (`lib/agent/stream.ts`), so the panel reads one spelling.

import type { AgentMessageRole } from "./enums";

// The server's own ceiling, in characters after trim. The console refuses past
// it too, so a message it forwards is never one the server would turn away.
export const AGENT_MESSAGE_MAX = 4000;

// How long one turn may stream, the reply's body included. The server ends a
// turn and saves it first (`STREAM_MAX` in the wire contract), so this cut-off
// is only the backstop, and the contract test holds it the longer.
export const AGENT_STREAM_TIMEOUT_MS = 120_000;

export interface AgentCitation {
  index: number;
  title: string;
  // `null` for a source with no page of its own: an earlier question.
  url: string | null;
}

export interface AgentMessage {
  id: string;
  role: AgentMessageRole;
  content: string;
  citations: AgentCitation[];
}

export interface AgentConversationSummary {
  id: string;
  title: string;
  createdAt: string;
  updatedAt: string;
}

export interface AgentConversation {
  id: string;
  title: string;
  messages: AgentMessage[];
}

export type AgentStatus = "enabled" | "disabled" | "unavailable";
