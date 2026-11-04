import { describe, expect, it } from "vitest";

import { NEW_OBJECTIVE, lastsWords, objectiveDraftOf, objectiveInputOf, type ObjectiveDraft } from "./objectives";
import type { Alert } from "./server/data";
import { RuleLevel, RuleMeasure, Severity, SpanKind } from "./types/enums";

const DRAFT: ObjectiveDraft = { ...NEW_OBJECTIVE, name: " Support answers ", agent: "support-bot", target: "99.9" };

describe("objectiveInputOf", () => {
  it("sends one agent's burn alert, its thresholds the lane's, its target a share", () => {
    const checked = objectiveInputOf(DRAFT);
    expect(checked).toMatchObject({
      ok: true,
      input: {
        name: "Support answers",
        kind: SpanKind.Run,
        measure: RuleMeasure.Burn,
        filters: { agent: ["support-bot"] },
        alert_threshold: 14.4,
        warning_threshold: 6,
        objective: { target: 0.999, latency_ms: null, days: 30 },
      },
    });
  });

  it("takes a latency in whole milliseconds, and refuses what the lane would", () => {
    const latency = objectiveInputOf({ ...DRAFT, promise: "latency", latencyMs: "2,000", days: 7 });
    expect(latency.ok && latency.input.objective).toEqual({ target: 0.999, latency_ms: 2000, days: 7 });
    for (const refused of [
      { name: " " },
      { agent: "" },
      { agent: "a, b" },
      { target: "100" },
      { target: "40" },
      { promise: "latency" as const, latencyMs: "1.5" },
      { renotifyMinutes: "0" },
    ]) {
      expect(objectiveInputOf({ ...DRAFT, ...refused }).ok, JSON.stringify(refused)).toBe(false);
    }
  });

  it("reads a saved objective back into the form", () => {
    const alert = {
      id: "7a1c0e52-0000-4000-8000-000000000009",
      name: "Fast answers",
      kind: SpanKind.Run,
      measure: RuleMeasure.Burn,
      filters: { agent: ["support-bot"] },
      objective: { target: 0.95, latency_ms: 2000, days: 28 },
      renotify_minutes: 60,
      severity: Severity.Ok,
      connectors: [{ connection_id: "c1", level: RuleLevel.Alert, hours: null, muted: false }],
    } as unknown as Alert;
    expect(objectiveDraftOf(alert)).toEqual({
      name: "Fast answers",
      agent: "support-bot",
      promise: "latency",
      target: "95",
      latencyMs: "2000",
      days: 28,
      renotifyMinutes: "60",
      connectors: [{ connectionId: "c1", level: RuleLevel.Alert, hours: null }],
    });
  });

  it("says how long a budget lasts at a burn", () => {
    expect(lastsWords(30, 15)).toBe("2.0 days");
    expect(lastsWords(30, 300)).toBe("2 hours");
    expect(lastsWords(30, 0)).toBeNull();
  });
});
