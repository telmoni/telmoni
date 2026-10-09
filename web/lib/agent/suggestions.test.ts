import { describe, expect, it } from "vitest";

import { AGENT_MESSAGE_MAX } from "@/lib/types/agent";
import { Role } from "@/lib/types/enums";

import { agentSuggestions } from "./suggestions";

describe("agentSuggestions", () => {
  it("offers the audit log only to a role that may read it", () => {
    const topics = (role: Role | null) => agentSuggestions(role).map((s) => s.topic);
    expect(topics(Role.Owner)).toEqual(["members", "deliveries", "audit"]);
    expect(topics(Role.Admin)).toEqual(["members", "deliveries", "audit"]);
    expect(topics(Role.Member)).toEqual(["members", "deliveries"]);
    expect(topics(null)).toEqual(["members", "deliveries"]);
  });

  it("asks within the agent's message ceiling, each starter once", () => {
    const all = agentSuggestions(Role.Owner);
    expect(new Set(all.map((s) => s.question)).size).toBe(all.length);
    for (const s of all) {
      expect(s.question.trim()).toBe(s.question);
      expect(s.question.length).toBeGreaterThan(0);
      expect(s.question.length).toBeLessThanOrEqual(AGENT_MESSAGE_MAX);
    }
  });
});
