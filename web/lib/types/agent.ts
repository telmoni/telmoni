// The agent's conversation as the console holds it, camelCased from the
// server's wire shape at the entity (`lib/server/entities/agent.ts`) and at
// the stream parser (`lib/agent/stream.ts`), so the panel reads one spelling.

import { z } from "zod";

import { AgentMessageRole } from "./enums";

// The server's own ceiling, in characters after trim. The console refuses past
// it too, so a message it forwards is never one the server would turn away.
export const AGENT_MESSAGE_MAX = 4000;

// How long one turn may stream, the reply's body included. The server ends a
// turn and saves it first (`STREAM_MAX` in the wire contract), so this cut-off
// is only the backstop, and the contract test holds it the longer.
export const AGENT_STREAM_TIMEOUT_MS = 120_000;

export const AgentCitationSchema = z.object({
  index: z.number(),
  title: z.string(),
  /** `null` for a source with no page of its own: an earlier question. */
  url: z.string().nullable(),
});
export type AgentCitation = z.infer<typeof AgentCitationSchema>;

export const AgentMessageSchema = z.object({
  id: z.string(),
  role: z.enum(AgentMessageRole),
  content: z.string(),
  citations: z.array(AgentCitationSchema),
});
export type AgentMessage = z.infer<typeof AgentMessageSchema>;

export const AgentConversationSummarySchema = z.object({
  id: z.string(),
  title: z.string(),
  createdAt: z.string(),
  updatedAt: z.string(),
});
export type AgentConversationSummary = z.infer<typeof AgentConversationSummarySchema>;

export const AgentConversationSchema = z.object({
  id: z.string(),
  title: z.string(),
  messages: z.array(AgentMessageSchema),
});
export type AgentConversation = z.infer<typeof AgentConversationSchema>;

export const AgentStatusSchema = z.enum(["enabled", "disabled", "unavailable"]);
export type AgentStatus = z.infer<typeof AgentStatusSchema>;
