// The agent's conversation as the console holds it, camelCased from the
// server's wire shape at the entity (`lib/server/entities/agent.ts`) and at
// the stream parser (`lib/agent/stream.ts`), so the panel reads one spelling.

// The server's own ceiling, after trim. The console refuses past it too, so a
// message it forwards is never one the server would turn away for length.
export const AGENT_MESSAGE_MAX = 4000;

export interface AgentCitation {
  index: number;
  title: string;
  // `null` for a source with no page of its own: an earlier question.
  url: string | null;
}

export const AGENT_ROLES = ["user", "assistant"] as const;
export type AgentRole = (typeof AGENT_ROLES)[number];

// The server's `Tool` enum (`crates/agent/src/tools.rs`), by the names the
// model calls them by.
export const AGENT_TOOLS = [
  "search",
  "list_members",
  "list_connectors",
  "connector_deliveries",
  "audit_events",
] as const;
export type AgentTool = (typeof AGENT_TOOLS)[number];

// The agent's own problem types, which the panel words itself.
export const AGENT_PROBLEM = {
  disabled: "/errors/agent/disabled",
  rateLimited: "/errors/agent/rate-limited",
  modelUnavailable: "/errors/agent/model-unavailable",
} as const;

export interface AgentMessage {
  id: string;
  role: AgentRole;
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
