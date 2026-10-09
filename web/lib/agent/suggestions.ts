import { Role } from "@/lib/types/enums";

export type SuggestionTopic = "members" | "deliveries" | "audit";

export interface AgentSuggestion {
  topic: SuggestionTopic;
  title: string;
  description: string;
  /** Sent as it stands, as if typed. */
  question: string;
}

const MEMBERS: AgentSuggestion = {
  topic: "members",
  title: "Who has access",
  description: "The project's members and the role each holds",
  question: "Who are this project's members, and what role does each of them hold?",
};

const DELIVERIES: AgentSuggestion = {
  topic: "deliveries",
  title: "Check deliveries",
  description: "Whether its connectors' notices are getting through",
  question: "Are this project's connectors delivering? Tell me about any recent failures.",
};

const AUDIT: AgentSuggestion = {
  topic: "audit",
  title: "Review recent changes",
  description: "What the project's audit log shows lately",
  question: "What has changed in this project lately, according to its audit log?",
};

// An empty conversation's starters, one for each thing the agent's tools read
// (`crates/agent/src/tools.rs`). The audit log is offered only to a role that
// may read it — Admin and up, `can(role, Read, Audit)` in
// `crates/shared/src/rbac.rs` — since the agent would answer anyone else's
// question with a refusal.
export function agentSuggestions(role: Role | null): AgentSuggestion[] {
  const auditor = role === Role.Owner || role === Role.Admin;
  return auditor ? [MEMBERS, DELIVERIES, AUDIT] : [MEMBERS, DELIVERIES];
}
