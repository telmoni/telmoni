import { describe, expect, it } from "vitest";

import {
  ALERT_KINDS,
  NEW_ALERT,
  amount,
  connectionChoices,
  draftOf,
  filterFieldsFor,
  hoursWords,
  incidentHref,
  incidentKindsLabel,
  inForce,
  inputOf,
  joinValues,
  memberNames,
  measuresFor,
  metricWords,
  offsetWords,
  renotifyWords,
  splitValues,
  thresholdWords,
  toldWords,
  windowsFor,
  withKind,
  withMeasure,
  type AlertDraft,
} from "./alerts";
import type { Alert, Mute } from "./server/data";
import {
  MissingData,
  NotificationKind,
  RuleAggregation,
  RuleLevel,
  RuleMeasure,
  RuleOperator,
  RuleWindow,
  Severity,
  SpanKind,
  SpanStatus,
} from "./types/enums";

const ALERT: Alert = {
  id: "7a1c0e52-0000-4000-8000-000000000002",
  name: "Slow calls",
  kind: SpanKind.ModelCall,
  measure: RuleMeasure.Duration,
  aggregation: RuleAggregation.P95,
  filters: { model: ["claude-sonnet-5-5"], status: [SpanStatus.Error] },
  window: RuleWindow.Hour,
  operator: RuleOperator.Gt,
  alert_threshold: 5000,
  warning_threshold: 3000,
  missing_data: MissingData.Keep,
  renotify_minutes: 30,
  paused: null,
  paused_at: null,
  value: 6200,
  evaluated_at: "2026-10-26T03:00:00Z",
  severity: Severity.Alert,
  severity_since: "2026-10-26T03:00:00Z",
  connectors: [
    {
      connection_id: "3c4e2a74-0000-4000-8000-000000000004",
      level: RuleLevel.Alert,
      hours: { days: [1, 2, 3, 4, 5], from: "09:00", to: "18:00", utc_offset: 60 },
      muted: false,
    },
  ],
  created_at: "2026-10-25T03:00:00Z",
  updated_at: "2026-10-25T03:00:00Z",
};

function draft(over: Partial<AlertDraft> = {}): AlertDraft {
  return { ...draftOf(ALERT), ...over };
}

describe("what an alert measures", () => {
  it("measures tokens on model calls alone and cost on runs alone", () => {
    for (const kind of ALERT_KINDS) {
      expect(measuresFor(kind).includes(RuleMeasure.Tokens)).toBe(kind === SpanKind.ModelCall);
      expect(measuresFor(kind).includes(RuleMeasure.Cost)).toBe(kind === SpanKind.Run);
    }
  });

  it("keeps a measure and a fold the kind still offers, and falls back where it does not", () => {
    expect(withMeasure({ ...NEW_ALERT }, RuleMeasure.Cost).aggregation).toBe(RuleAggregation.Avg);
    const cost = withMeasure(draft(), RuleMeasure.Cost);
    expect(cost.aggregation).toBe(RuleAggregation.Max);
    expect(withKind(cost, SpanKind.Step)).toMatchObject({
      measure: RuleMeasure.Count,
      aggregation: RuleAggregation.Count,
    });
    expect(withKind(draft(), SpanKind.ToolCall)).toMatchObject({
      measure: RuleMeasure.Duration,
      aggregation: RuleAggregation.P95,
    });
  });

  it("reads a metric in its notices' words", () => {
    expect(metricWords(ALERT)).toBe("The p95 duration of model calls over the last hour");
    expect(
      metricWords({ kind: SpanKind.Run, measure: RuleMeasure.Failures, aggregation: RuleAggregation.Avg, window: RuleWindow.Day }),
    ).toBe("The share failed of runs over the last day");
    expect(
      metricWords({ kind: SpanKind.Run, measure: RuleMeasure.Cost, aggregation: RuleAggregation.Max, window: RuleWindow.Week }),
    ).toBe("The most a run spent over the last week");
    expect(
      metricWords({ kind: SpanKind.ToolCall, measure: RuleMeasure.Count, aggregation: RuleAggregation.Count, window: RuleWindow.Hour }),
    ).toBe("The count of tool calls over the last hour");
  });

  it("writes a value in its unit", () => {
    expect(amount(RuleMeasure.Duration, RuleAggregation.P95, 6200)).toBe("6,200 ms");
    expect(amount(RuleMeasure.Failures, RuleAggregation.Avg, 0.125)).toBe("12.5%");
    expect(amount(RuleMeasure.Failures, RuleAggregation.Count, 3)).toBe("3");
    expect(amount(RuleMeasure.Cost, RuleAggregation.Max, 3.45)).toBe("$3.45");
    expect(thresholdWords(ALERT)).toEqual({ alert: "> 5,000 ms", warning: "> 3,000 ms" });
  });
});

describe("the form", () => {
  // ⚠ Saving an alert unchanged sends what it holds.
  it("turns an alert into its form and back unchanged", () => {
    const checked = inputOf(draftOf(ALERT));
    expect(checked).toEqual({
      ok: true,
      input: {
        name: "Slow calls",
        kind: SpanKind.ModelCall,
        measure: RuleMeasure.Duration,
        aggregation: RuleAggregation.P95,
        filters: { model: ["claude-sonnet-5-5"], status: [SpanStatus.Error] },
        window: RuleWindow.Hour,
        operator: RuleOperator.Gt,
        alert_threshold: 5000,
        warning_threshold: 3000,
        missing_data: MissingData.Keep,
        renotify_minutes: 30,
        connectors: [
          {
            connection_id: "3c4e2a74-0000-4000-8000-000000000004",
            level: RuleLevel.Alert,
            hours: { days: [1, 2, 3, 4, 5], from: "09:00", to: "18:00", utc_offset: 60 },
          },
        ],
      },
    });
  });

  it("types a share as a percentage, and sends it as the share", () => {
    const share: Alert = {
      ...ALERT,
      kind: SpanKind.Run,
      measure: RuleMeasure.Failures,
      aggregation: RuleAggregation.Avg,
      alert_threshold: 0.1,
      warning_threshold: 0.05,
    };
    expect(draftOf(share)).toMatchObject({ alertThreshold: "10", warningThreshold: "5" });
    const checked = inputOf(draft({ ...draftOf(share), alertThreshold: "12.5", warningThreshold: "" }));
    expect(checked.ok && checked.input).toMatchObject({ alert_threshold: 0.125, warning_threshold: null });
  });

  it("sends the filters the kind's spans carry, each value once", () => {
    const checked = inputOf(
      draft({
        kind: SpanKind.Run,
        measure: RuleMeasure.Count,
        aggregation: RuleAggregation.Count,
        filters: { ...draftOf(ALERT).filters, agent: " triage, support , triage,", model: "gpt-4o" },
      }),
    );
    expect(checked.ok && checked.input.filters).toEqual({
      agent: ["triage", "support"],
      status: [SpanStatus.Error],
    });
    expect(filterFieldsFor(SpanKind.ToolCall, RuleMeasure.Count)).toEqual([
      "agent",
      "tool",
      "environment",
      "release",
    ]);
  });

  // A run's cost is read from its calls as they land: their model, never a status.
  it("filters a run's cost by its calls' model and provider, never by status", () => {
    const checked = inputOf(
      draft({
        kind: SpanKind.Run,
        measure: RuleMeasure.Cost,
        aggregation: RuleAggregation.Max,
        alertThreshold: "5",
        warningThreshold: "",
        statuses: [SpanStatus.Error],
      }),
    );
    expect(checked.ok && checked.input.filters).toEqual({ model: ["claude-sonnet-5-5"] });
  });

  it("says what is wrong, in the words a person fixes it by", () => {
    const refused: [Partial<AlertDraft>, RegExp][] = [
      [{ name: "  " }, /Name the alert/],
      [{ name: "x".repeat(61) }, /1 to 60 characters/],
      [{ alertThreshold: "" }, /alert threshold is a number/],
      [{ alertThreshold: "fast" }, /alert threshold is a number/],
      [{ warningThreshold: "6000" }, /below the alert threshold/],
      [{ warningThreshold: "5000" }, /below the alert threshold/],
      [{ operator: RuleOperator.Lte, warningThreshold: "5000" }, /above the alert threshold/],
      [{ operator: RuleOperator.Lt, warningThreshold: "4000" }, /above the alert threshold/],
      [{ warningThreshold: "soon" }, /warning threshold is a number/],
      [{ renotifyMinutes: "0" }, /every 1 to 10,080 minutes/],
      [{ renotifyMinutes: "1.5" }, /every 1 to 10,080 minutes/],
      [
        { kind: SpanKind.Step, measure: RuleMeasure.Tokens, aggregation: RuleAggregation.Sum },
        /Tokens is not measured on steps/,
      ],
      [{ aggregation: RuleAggregation.Count }, /how the measure is folded/],
      [
        { connectors: [...draftOf(ALERT).connectors, ...draftOf(ALERT).connectors] },
        /links a connector once/,
      ],
      [
        { connectors: [{ connectionId: "c", level: RuleLevel.Warning, hours: { days: [], from: "09:00", to: "18:00", utc_offset: 0 } }] },
        /name a day/,
      ],
      [
        { connectors: [{ connectionId: "c", level: RuleLevel.Warning, hours: { days: [1], from: "9:00", to: "18:00", utc_offset: 0 } }] },
        /time of day/,
      ],
      [
        {
          connectors: Array.from({ length: 21 }, (_, i) => ({
            connectionId: String(i),
            level: RuleLevel.Warning,
            hours: null,
          })),
        },
        /up to 20 connectors/,
      ],
    ];
    for (const [over, error] of refused) {
      const checked = inputOf(draft(over));
      expect(checked.ok, JSON.stringify(over)).toBe(false);
      if (!checked.ok) expect(checked.error).toMatch(error);
    }
  });

  it("takes no warning under an equality, and grouping commas in a number", () => {
    const checked = inputOf(draft({ operator: RuleOperator.Eq, alertThreshold: "5,000" }));
    expect(checked.ok && checked.input).toMatchObject({ alert_threshold: 5000, warning_threshold: null });
    const grouped = inputOf(draft({ alertThreshold: "12,500.5", warningThreshold: "1,000" }));
    expect(grouped.ok && grouped.input).toMatchObject({ alert_threshold: 12500.5, warning_threshold: 1000 });
  });

  // "1,440" is a day, its comma read as a threshold's is.
  it("takes grouping commas in Notify again, still a whole number of minutes", () => {
    const day = inputOf(draft({ renotifyMinutes: "1,440" }));
    expect(day.ok && day.input.renotify_minutes).toBe(1440);
    const week = inputOf(draft({ renotifyMinutes: " 10,080 " }));
    expect(week.ok && week.input.renotify_minutes).toBe(10_080);
    for (const typed of ["1,5", "1,440.5", "10,081"]) {
      const checked = inputOf(draft({ renotifyMinutes: typed }));
      expect(checked.ok, typed).toBe(false);
      if (!checked.ok) expect(checked.error).toMatch(/every 1 to 10,080 minutes/);
    }
  });

  // A decimal comma is no grouping: "0,5" is not five.
  it("refuses a comma that does not group thousands", () => {
    for (const typed of ["0,5", "1,5", "12,50", "1,00,000"]) {
      const checked = inputOf(draft({ alertThreshold: typed }));
      expect(checked.ok, typed).toBe(false);
      if (!checked.ok) expect(checked.error).toMatch(/decimals after a point/);
    }
    const warning = inputOf(draft({ warningThreshold: "0,5" }));
    expect(!warning.ok && warning.error).toMatch(/warning threshold is a number/);
  });

  it("splits typed values on commas, each once", () => {
    expect(splitValues(" a, b ,, a ,c")).toEqual(["a", "b", "c"]);
    expect(splitValues("   ")).toEqual([]);
  });

  // A value may hold a comma, or a quote: the field still types it.
  it("reads a value between double quotes whole, and writes it so", () => {
    expect(splitValues('"support, tier 2", triage')).toEqual(["support, tier 2", "triage"]);
    expect(splitValues('"say ""hi""", plain"quote')).toEqual(['say "hi"', 'plain"quote']);
    for (const values of [["support, tier 2", "triage"], ['say "hi"', "a,b,c", "plain"], ['"lead']]) {
      expect(splitValues(joinValues(values))).toEqual(values);
    }
    const quoted: Alert = { ...ALERT, filters: { agent: ["support, tier 2", "triage"] } };
    const checked = inputOf(draftOf(quoted));
    expect(checked.ok && checked.input.filters).toEqual({ agent: ["support, tier 2", "triage"] });
  });
});

describe("words", () => {
  it("reads hours as a person does", () => {
    expect(hoursWords({ days: [5, 1, 2, 3, 4], from: "09:00", to: "18:00", utc_offset: 60 })).toBe(
      "Mon–Fri 09:00–18:00 UTC+01:00",
    );
    expect(hoursWords({ days: [6, 7], from: "22:00", to: "06:00", utc_offset: -330 })).toBe(
      "Sat, Sun 22:00–06:00 UTC−05:30",
    );
    expect(hoursWords({ days: [1, 2, 3, 4, 5, 6, 7], from: "00:00", to: "00:00", utc_offset: 0 })).toBe(
      "Every day all day UTC",
    );
    expect(offsetWords(345)).toBe("UTC+05:45");
  });

  it("reads how often a held breach is told again", () => {
    expect(renotifyWords(null)).toBe("Never");
    expect(renotifyWords(1)).toBe("Every minute");
    expect(renotifyWords(45)).toBe("Every 45 minutes");
    expect(renotifyWords(60)).toBe("Every hour");
    expect(renotifyWords(240)).toBe("Every 4 hours");
    expect(renotifyWords(10_080)).toBe("Every 7 days");
  });

  it("names where an incident's notices went, the feed first", () => {
    const names = new Map([["c1", "#alerts in Acme"]]);
    expect(toldWords({ told_to: ["c1", "c2"] }, names, "a removed connector")).toEqual([
      "the feed",
      "#alerts in Acme",
      "a removed connector",
    ]);
    expect(toldWords({ told_to: [] }, names, "a removed connector")).toEqual(["the feed"]);
  });

  // The form's switches and where an incident's notices went name each one apart.
  it("tells two webhooks on one host apart by their fingerprints", () => {
    const connection = (id: string, provider: "slack" | "webhook", channel: string, workspace: string | null) => ({
      id,
      provider,
      external_workspace_id: "w",
      external_workspace_name: workspace,
      channel_id: `${id}-fingerprint`,
      channel_name: channel,
      status: "active" as const,
      last_error: null,
      last_delivery_at: null,
      created_at: "2026-10-01T00:00:00Z",
      event_kinds: null,
      previous_secret_expires_at: null,
    });
    expect(
      connectionChoices([
        connection("abcd1234", "webhook", "hooks.example.com", null),
        connection("efgh5678", "webhook", "hooks.example.com", null),
        connection("ijkl9012", "webhook", "other.example.com", null),
        connection("mnop3456", "slack", "alerts", "Acme"),
      ]).map((choice) => choice.name),
    ).toEqual([
      "hooks.example.com (abcd1234)",
      "hooks.example.com (efgh5678)",
      "other.example.com",
      "#alerts in Acme",
    ]);
  });

  // ⚠ Notifications sends a webhook only the kinds it takes, an alert's incidents among them.
  it("says what a webhook takes of an alert's incidents, where not both kinds", () => {
    const { IncidentOpened, IncidentResolved, MemberAdded, MemberLeft } = NotificationKind;
    const [choice] = connectionChoices([
      {
        id: "abcd1234",
        provider: "webhook",
        external_workspace_id: "w",
        external_workspace_name: null,
        channel_id: "abcd1234-fingerprint",
        channel_name: "hooks.example.com",
        status: "active",
        last_error: null,
        last_delivery_at: null,
        created_at: "2026-10-01T00:00:00Z",
        event_kinds: [IncidentOpened, MemberAdded],
        previous_secret_expires_at: null,
      },
    ]);
    expect(choice?.eventKinds).toEqual([IncidentOpened, MemberAdded]);
    expect(incidentKindsLabel(null)).toBeNull();
    expect(incidentKindsLabel([MemberAdded, IncidentResolved, IncidentOpened])).toBeNull();
    expect(incidentKindsLabel([])).toBe("Takes no incidents");
    expect(incidentKindsLabel([MemberAdded, MemberLeft])).toBe("Takes no incidents");
    expect(incidentKindsLabel([IncidentOpened, MemberAdded])).toBe("Takes openings only");
    expect(incidentKindsLabel([IncidentResolved])).toBe("Takes resolutions only");
  });

  it("links an incident on the alerts page, drawn and scrolled to", () => {
    expect(incidentHref("acme", "web", "5d4f5c68-0000-4000-8000-000000000001")).toBe(
      "/acme/web/alerts?incident=5d4f5c68-0000-4000-8000-000000000001#incident-5d4f5c68-0000-4000-8000-000000000001",
    );
  });

  // ⚠ An organization's admin acts on every project with no seat on it.
  it("names a member from the rosters read, and says what the rest may be", () => {
    const ada = { member_id: "user_1", display_name: "Ada", email: "ada@example.test" };
    const sam = { member_id: "user_2", display_name: null, email: "sam@example.test" };
    const grace = { member_id: "user_3", display_name: "Grace", email: "grace@example.test" };
    const both = memberNames([grace], [ada, sam]);
    expect([both("user_1"), both("user_2"), both("user_3"), both("user_9")]).toEqual([
      "Ada",
      "sam@example.test",
      "Grace",
      "a former member",
    ]);
    expect(memberNames(null, [ada])("user_9")).toBe("an organization admin or a former member");
    expect(memberNames(null, null)("user_1")).toBe("someone");
  });
});

describe("maintenance windows", () => {
  const now = Date.parse("2026-10-26T12:00:00Z");
  const mute = (id: string, rule: string | null, from: string, to: string): Mute => ({
    id,
    rule_id: rule,
    starts_at: from,
    ends_at: to,
    created_by: "user_1",
    created_at: from,
    ended_by: null,
  });
  const mutes = [
    mute("over", ALERT.id, "2026-10-26T09:00:00Z", "2026-10-26T11:00:00Z"),
    mute("later", ALERT.id, "2026-10-27T09:00:00Z", "2026-10-27T11:00:00Z"),
    mute("now", ALERT.id, "2026-10-26T11:00:00Z", "2026-10-26T13:00:00Z"),
    mute("all", null, "2026-10-26T11:30:00Z", "2026-10-26T14:00:00Z"),
    mute("other", "another", "2026-10-26T11:00:00Z", "2026-10-26T13:00:00Z"),
  ];

  // ⚠ A window over every alert mutes each one, so each one lists it.
  it("lists the windows not yet over that mute an alert, its project's among them", () => {
    expect(windowsFor(mutes, ALERT.id, now).map((m) => m.id)).toEqual(["now", "all", "later"]);
    expect(windowsFor(mutes, null, now).map((m) => m.id)).toEqual(["all"]);
  });

  // The lane ends one still to start a moment after its start, still ahead.
  it("leaves out a window a member ended, even one still to start", () => {
    const ended = {
      ...mute("ended", ALERT.id, "2026-10-27T09:00:00Z", "2026-10-27T09:00:00.000001Z"),
      ended_by: "user_2",
    };
    expect(windowsFor([...mutes, ended], ALERT.id, now).map((m) => m.id)).toEqual([
      "now",
      "all",
      "later",
    ]);
  });

  it("tells one in force from one still to start", () => {
    expect(inForce(mutes[2]!, now)).toBe(true);
    expect(inForce(mutes[1]!, now)).toBe(false);
    expect(inForce(mutes[0]!, now)).toBe(false);
  });
});
