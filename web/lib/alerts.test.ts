import { describe, expect, it } from "vitest";

import {
  ALERT_KINDS,
  NEW_ALERT,
  alertWindowsFor,
  amount,
  beforeWords,
  changesNameAgent,
  connectionChoices,
  draftOf,
  filterFieldsFor,
  filterWords,
  filtersStatus,
  hoursWords,
  incidentHref,
  incidentKindsLabel,
  inForce,
  inputOf,
  joinValues,
  measuresFor,
  metricWords,
  offsetWords,
  renotifyWords,
  splitValues,
  starterWords,
  suggestionFor,
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
  RuleBaseline,
  RuleLevel,
  RuleMeasure,
  RuleOperator,
  RuleWindow,
  Severity,
  SpanKind,
  SpanFault,
  SpanStatus,
} from "./types/enums";

describe("fault alert selection", () => {
  const draft = { ...NEW_ALERT, name: "Rate limits", kind: SpanKind.ModelCall, measure: RuleMeasure.Faults,
    aggregation: RuleAggregation.Avg, alertThreshold: "5", faults: [SpanFault.RateLimited] };

  it("keeps category selection and converts a percent to a share", () => {
    const result = inputOf(draft);
    expect(result.ok).toBe(true);
    if (!result.ok) throw new Error(result.error);
    expect(result.input.filters.fault).toEqual([SpanFault.RateLimited]);
    expect(result.input.alert_threshold).toBe(0.05);
    expect(amount(RuleMeasure.Faults, RuleAggregation.Avg, 0.05)).toBe("5%");
    const edited = draftOf({ ...ALERT, ...result.input, id: ALERT.id, connectors: [] });
    expect(edited.faults).toEqual([SpanFault.RateLimited]);
    expect(edited.alertThreshold).toBe("5");
  });

  it("requires a category and omits the selector when changing measure", () => {
    expect(inputOf({ ...draft, faults: [] })).toEqual({ ok: false, error: "Choose at least one fault category." });
    const result = inputOf(withMeasure(draft, RuleMeasure.Failures));
    expect(result.ok).toBe(true);
    if (!result.ok) throw new Error(result.error);
    expect(result.input.filters.fault).toBeUndefined();
  });
});

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
  score_name: null,
  baseline: RuleBaseline.None,
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
  starter: false,
  created_at: "2026-10-25T03:00:00Z",
  updated_at: "2026-10-25T03:00:00Z",
};

function draft(over: Partial<AlertDraft> = {}): AlertDraft {
  return { ...draftOf(ALERT), ...over };
}

describe("what an alert measures", () => {
  it("measures tokens on model calls alone and cost on traces alone", () => {
    for (const kind of ALERT_KINDS) {
      expect(measuresFor(kind).includes(RuleMeasure.Tokens)).toBe(kind === SpanKind.ModelCall);
      expect(measuresFor(kind).includes(RuleMeasure.Cost)).toBe(kind === SpanKind.Agent);
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
      metricWords({ kind: SpanKind.Agent, measure: RuleMeasure.Failures, aggregation: RuleAggregation.Avg, window: RuleWindow.Day }),
    ).toBe("The share failed of traces over the last day");
    expect(
      metricWords({ kind: SpanKind.Agent, measure: RuleMeasure.Cost, aggregation: RuleAggregation.Max, window: RuleWindow.Week }),
    ).toBe("The most a trace spent over the last week");
    expect(
      metricWords({ kind: SpanKind.ToolCall, measure: RuleMeasure.Count, aggregation: RuleAggregation.Count, window: RuleWindow.Hour }),
    ).toBe("The count of tool calls over the last hour");
  });

  it("writes a value in its unit", () => {
    expect(amount(RuleMeasure.Duration, RuleAggregation.P95, 6200)).toBe("6,200 ms");
    expect(amount(RuleMeasure.Failures, RuleAggregation.Avg, 0.125)).toBe("12.5%");
    expect(amount(RuleMeasure.Failures, RuleAggregation.Count, 3)).toBe("3");
    expect(amount(RuleMeasure.Cost, RuleAggregation.Max, 3.45)).toBe("$3.45");
    expect(
      metricWords({ kind: SpanKind.Agent, measure: RuleMeasure.Cost, aggregation: RuleAggregation.Sum, window: RuleWindow.Month }),
      "a budget",
    ).toBe("What the traces spent so far this month");
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
        score_name: null,
        baseline: RuleBaseline.None,
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
      kind: SpanKind.Agent,
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
        kind: SpanKind.Agent,
        measure: RuleMeasure.Count,
        aggregation: RuleAggregation.Count,
        filters: { ...draftOf(ALERT).filters, agent: " triage, support , triage,", model: "gpt-4o" },
      }),
    );
    expect(checked.ok && checked.input.filters).toEqual({
      agent: ["triage", "support"],
      status: [SpanStatus.Error],
    });
    expect(filterFieldsFor(SpanKind.Agent, RuleMeasure.Cost), "a budget by its tag").toEqual([
      "agent",
      "model",
      "provider",
      "environment",
      "release",
      "tag",
    ]);
    expect(filterFieldsFor(SpanKind.Agent, RuleMeasure.Count)).not.toContain("tag");
    expect(filterFieldsFor(SpanKind.ToolCall, RuleMeasure.Count)).toEqual([
      "agent",
      "tool",
      "environment",
      "release",
    ]);
  });

  // A trace's cost is read from its calls as they land: their model, never a status.
  it("filters a trace's cost by its calls' model and provider, never by status", () => {
    const checked = inputOf(
      draft({
        kind: SpanKind.Agent,
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

describe("a retrieval alert", () => {
  const SEARCHES: Alert = {
    ...ALERT,
    name: "Failing docs searches",
    kind: SpanKind.Retrieval,
    measure: RuleMeasure.Failures,
    aggregation: RuleAggregation.Count,
    filters: { agent: ["support-bot"], source: ["docs-index"] },
  };

  it("counts, fails and times retrievals as tool calls, in its notices' words", () => {
    expect(measuresFor(SpanKind.Retrieval)).toEqual(measuresFor(SpanKind.ToolCall));
    expect(metricWords(SEARCHES)).toBe("The failures of retrievals over the last hour");
    expect(filterWords(SEARCHES.filters)).toEqual(["agent support-bot", "source docs-index"]);
  });

  // ⚠ A source lost on the way back would widen the alert to every retrieval as it is saved again.
  it("filters by source as a tool call by tool, and keeps it as it is saved again", () => {
    expect(filterFieldsFor(SpanKind.Retrieval, RuleMeasure.Count)).toEqual([
      "agent",
      "source",
      "environment",
      "release",
    ]);
    expect(filterFieldsFor(SpanKind.ToolCall, RuleMeasure.Count)).not.toContain("source");
    expect(draftOf(SEARCHES).filters.source).toBe("docs-index");
    const checked = inputOf(draftOf(SEARCHES));
    expect(checked.ok && checked.input).toMatchObject({
      kind: SpanKind.Retrieval,
      filters: { agent: ["support-bot"], source: ["docs-index"] },
    });
  });

  // The lane refuses a source on any other kind, and a tool on a retrieval.
  it("sends a source on a retrieval alone, and a tool on a tool call alone", () => {
    const typed = { ...draftOf(SEARCHES), filters: { ...draftOf(SEARCHES).filters, tool: "search_docs" } };
    const searches = inputOf(typed);
    expect(searches.ok && searches.input.filters).toEqual({ agent: ["support-bot"], source: ["docs-index"] });
    const tools = inputOf(withKind(typed, SpanKind.ToolCall));
    expect(tools.ok && tools.input.filters).toEqual({ agent: ["support-bot"], tool: ["search_docs"] });
  });
});

describe("a score alert", () => {
  const SCORED: AlertDraft = {
    ...NEW_ALERT,
    name: "Relevance dropped",
    measure: RuleMeasure.Score,
    scoreName: " relevance ",
    missingData: MissingData.NoData,
    window: RuleWindow.Day,
    baseline: RuleBaseline.Previous,
    operator: RuleOperator.Lt,
    alertThreshold: "-20",
    warningThreshold: "-10",
    filters: { ...NEW_ALERT.filters, agent: "bot" },
  };

  it("names its score, takes no filter, and sends a change in percent as typed", () => {
    const checked = inputOf(SCORED);
    expect(checked.ok && checked.input).toMatchObject({
      measure: RuleMeasure.Score,
      score_name: "relevance",
      baseline: RuleBaseline.Previous,
      filters: {},
      alert_threshold: -20,
      warning_threshold: -10,
    });
    expect(filterFieldsFor(SpanKind.Agent, RuleMeasure.Score)).toEqual([]);
    expect(filtersStatus(RuleMeasure.Score)).toBe(false);
  });

  it("reads an empty window as no data, where it is no 0, and refuses a 0 there", () => {
    const scored = withMeasure({ ...NEW_ALERT, kind: SpanKind.Agent }, RuleMeasure.Score);
    expect(scored.missingData).toBe(MissingData.NoData);
    expect(withMeasure({ ...scored, aggregation: RuleAggregation.Count }, RuleMeasure.Score).missingData).toBe(
      MissingData.NoData,
    );
    expect(inputOf({ ...SCORED, missingData: MissingData.Zero }).ok).toBe(false);
    expect(inputOf({ ...SCORED, aggregation: RuleAggregation.Count, missingData: MissingData.Zero }).ok).toBe(true);
  });

  it("refuses one with no score, or a month against the one before", () => {
    expect(inputOf({ ...SCORED, scoreName: "  " }).ok).toBe(false);
    expect(inputOf({ ...SCORED, scoreName: "n".repeat(65) }).ok).toBe(false);
    expect(inputOf({ ...SCORED, window: RuleWindow.Month }).ok).toBe(false);
  });

  it("sends no score's name for any other measure", () => {
    const checked = inputOf({ ...SCORED, measure: RuleMeasure.Count, aggregation: RuleAggregation.Count });
    expect(checked.ok && checked.input.score_name).toBeNull();
  });

  it("reads in its notices' words, and a change as a signed percentage", () => {
    const metric = {
      kind: SpanKind.Agent,
      measure: RuleMeasure.Score,
      aggregation: RuleAggregation.Avg,
      window: RuleWindow.Day,
      score_name: "relevance",
      baseline: RuleBaseline.Previous,
    };
    expect(metricWords(metric)).toBe("The average relevance score over the last day, against the day before");
    expect(metricWords({ ...metric, aggregation: RuleAggregation.P90, baseline: RuleBaseline.None })).toBe(
      "The p90 relevance score over the last day",
    );
    expect(amount(RuleMeasure.Score, RuleAggregation.Avg, -12.5, RuleBaseline.Previous)).toBe("-12.5%");
    expect(amount(RuleMeasure.Score, RuleAggregation.Avg, 0.8125)).toBe("0.81");
  });
});

describe("loops and stalls", () => {
  const LOOPS: AlertDraft = {
    ...NEW_ALERT,
    name: "Loops",
    measure: RuleMeasure.Repeats,
    aggregation: RuleAggregation.Max,
    operator: RuleOperator.Gte,
    alertThreshold: "12",
    filters: { ...NEW_ALERT.filters, agent: "support-agent", tool: "search", model: "gpt-4o" },
    statuses: [SpanStatus.Error],
  };
  const LOOPING = [RuleMeasure.Steps, RuleMeasure.Repeats, RuleMeasure.Stalled];

  it("reads a trace's steps, repeats and stalls on traces alone", () => {
    for (const kind of ALERT_KINDS) {
      for (const measure of LOOPING) expect(measuresFor(kind).includes(measure)).toBe(kind === SpanKind.Agent);
    }
  });

  // Read as a trace's spans land, before the agent's own status is known.
  it("filters them by agent, environment and release, never by status", () => {
    const checked = inputOf(LOOPS);
    expect(checked.ok && checked.input).toMatchObject({ measure: RuleMeasure.Repeats, filters: { agent: ["support-agent"] } });
    for (const measure of LOOPING) {
      expect(filterFieldsFor(SpanKind.Agent, measure)).toEqual(["agent", "environment", "release"]);
      expect(filtersStatus(measure)).toBe(false);
    }
  });

  it("reads a stall over the last hour, day or week, never the month so far", () => {
    expect(alertWindowsFor(RuleMeasure.Stalled)).toEqual([RuleWindow.Hour, RuleWindow.Day, RuleWindow.Week]);
    expect(alertWindowsFor(RuleMeasure.Steps)).toContain(RuleWindow.Month);
    const monthly = { ...NEW_ALERT, window: RuleWindow.Month };
    expect(withMeasure(monthly, RuleMeasure.Stalled)).toMatchObject({
      aggregation: RuleAggregation.Count,
      window: RuleWindow.Hour,
    });
    expect(withMeasure(monthly, RuleMeasure.Steps).window).toBe(RuleWindow.Month);
    const stalled = { ...LOOPS, measure: RuleMeasure.Stalled, aggregation: RuleAggregation.Count };
    expect(inputOf(stalled).ok).toBe(true);
    expect(inputOf({ ...stalled, window: RuleWindow.Month })).toEqual({
      ok: false,
      error: "A stall is a trace quiet past the window: the last hour, day or week.",
    });
  });

  it("reads in its notices' words", () => {
    const trace = { kind: SpanKind.Agent, window: RuleWindow.Hour };
    expect(metricWords({ ...trace, measure: RuleMeasure.Repeats, aggregation: RuleAggregation.Max })).toBe(
      "The most times a trace called one tool in a row over the last hour",
    );
    expect(metricWords({ ...trace, measure: RuleMeasure.Steps, aggregation: RuleAggregation.P95, window: RuleWindow.Day })).toBe(
      "The p95 of the steps a trace took over the last day",
    );
    expect(metricWords({ ...trace, measure: RuleMeasure.Steps, aggregation: RuleAggregation.Max })).toBe(
      "The most steps a trace took over the last hour",
    );
    expect(metricWords({ ...trace, measure: RuleMeasure.Stalled, aggregation: RuleAggregation.Count })).toBe(
      "The count of traces left open and quiet for over an hour",
    );
    expect(
      metricWords({ ...trace, measure: RuleMeasure.Stalled, aggregation: RuleAggregation.Count, window: RuleWindow.Week }),
    ).toBe("The count of traces left open and quiet for over a week");
    expect(amount(RuleMeasure.Repeats, RuleAggregation.Max, 12)).toBe("12");
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

  // ⚠ The words its notice says, so the page and the notice read alike.
  it("says how long before its incident opened a change came", () => {
    const opened = "2026-10-26T12:00:00Z";
    const before = (seconds: number) =>
      beforeWords(opened, new Date(Date.parse(opened) - seconds * 1000).toISOString());
    expect(before(0)).toBe("under a minute before");
    expect(before(59)).toBe("under a minute before");
    expect(before(60)).toBe("1 minute before");
    expect(before(14 * 60)).toBe("14 minutes before");
    expect(before(59 * 60 + 59)).toBe("59 minutes before");
    expect(before(60 * 60)).toBe("1 hour before");
    expect(before(3 * 60 * 60)).toBe("3 hours before");
    expect(before(23 * 60 * 60 + 59 * 60)).toBe("23 hours before");
  });

  it("names a change's agent on the incidents of an alert held to no single agent", () => {
    expect(changesNameAgent({ filters: {} })).toBe(true);
    expect(changesNameAgent({ filters: { agent: [] } })).toBe(true);
    expect(changesNameAgent({ filters: { agent: ["support", "triage"] } })).toBe(true);
    expect(changesNameAgent({ filters: { agent: ["support"] } })).toBe(false);
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
        connection("qrst7890", "webhook", "named.example.com", "On-call bot"),
      ]).map((choice) => choice.name),
    ).toEqual([
      "hooks.example.com (abcd1234)",
      "hooks.example.com (efgh5678)",
      "other.example.com",
      "#alerts in Acme",
      "On-call bot",
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
});

describe("maintenance windows", () => {
  const now = Date.parse("2026-10-26T12:00:00Z");
  const mute = (id: string, rule: string | null, from: string, to: string): Mute => ({
    id,
    rule_id: rule,
    monitor_id: null,
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

describe("starterWords", () => {
  it("names the starters in a sentence", () => {
    expect(starterWords([RuleMeasure.Stalled])).toBe("Stalled traces");
    expect(starterWords([RuleMeasure.Failures, RuleMeasure.Repeats, RuleMeasure.Stalled])).toBe(
      "Failing traces, Looping traces and Stalled traces",
    );
  });
});

describe("suggestionFor", () => {
  const percentiles = (p50: number, p90: number, p99: number) => ({ p50, p90, p99 });
  const HISTORY = {
    from: "2026-10-21T00:00:00Z",
    to: "2026-11-04T00:00:00Z",
    agents: [
      {
        agent: "support-bot",
        kind: SpanKind.ModelCall,
        spans: 900,
        hours: 120,
        duration_ms: percentiles(1_200.4, 3_400.6, 8_100),
        tokens_hourly: percentiles(5_000, 12_000, 40_000),
        tokens_per_span: percentiles(300, 900, 2_000),
        spend_hourly: percentiles(0.123456, 0.5, 2),
        failure_share: percentiles(0, 0.1, 0.5),
      },
      {
        agent: null,
        kind: SpanKind.ModelCall,
        spans: 2_000,
        hours: 300,
        duration_ms: percentiles(900, 2_000, 6_000),
        tokens_hourly: percentiles(9_000, 20_000, 70_000),
        tokens_per_span: percentiles(250, 800, 1_800),
        spend_hourly: percentiles(0.2, 0.9, 3),
        failure_share: percentiles(0, 0.05, 0.25),
      },
      {
        agent: null,
        kind: SpanKind.Agent,
        spans: 400,
        hours: 200,
        duration_ms: percentiles(20_000, 60_000, 90_000),
        tokens_hourly: percentiles(0, 0, 0),
        tokens_per_span: percentiles(0, 0, 0),
        spend_hourly: percentiles(0, 0, 0),
        failure_share: percentiles(0, 0.034, 0.2),
      },
    ],
  };
  const draft = (over: Partial<AlertDraft>): AlertDraft => ({
    ...NEW_ALERT,
    kind: SpanKind.ModelCall,
    measure: RuleMeasure.Duration,
    aggregation: RuleAggregation.P95,
    ...over,
    filters: { ...NEW_ALERT.filters, ...over.filters },
  });

  it("offers the agent's own duration percentiles, whole, typed as the form types them", () => {
    const got = suggestionFor(draft({ filters: { ...NEW_ALERT.filters, agent: "support-bot" } }), HISTORY);
    expect(got?.who).toBe("support-bot");
    expect(got?.days).toBe(14);
    expect(got?.levels).toEqual([
      { level: "p50", shown: "1,200 ms", typed: "1200" },
      { level: "p90", shown: "3,401 ms", typed: "3401" },
      { level: "p99", shown: "8,100 ms", typed: "8100" },
    ]);
  });

  it("reads every agent's where none is named, and an hour's spend for an hour's sum", () => {
    expect(suggestionFor(draft({}), HISTORY)?.who).toBe("every agent");
    const spend = suggestionFor(
      draft({ kind: SpanKind.Agent, measure: RuleMeasure.Cost, aggregation: RuleAggregation.Sum }),
      HISTORY,
    );
    expect(spend?.levels[0]).toMatchObject({ level: "p50", typed: "0.2" });
  });

  it("offers the share of an hour's traces that failed in whole percents, up, as a starter's is set", () => {
    const got = suggestionFor(
      draft({ kind: SpanKind.Agent, measure: RuleMeasure.Failures, aggregation: RuleAggregation.Avg }),
      HISTORY,
    );
    expect(got?.what).toBe("the share of an hour's traces that failed");
    expect(got?.levels).toEqual([
      { level: "p50", shown: "0%", typed: "0" },
      { level: "p90", shown: "4%", typed: "4" },
      { level: "p99", shown: "20%", typed: "20" },
    ]);
    // A new alert's form starts on that measure, so it offers them first.
    expect(suggestionFor(NEW_ALERT, HISTORY)?.levels).toHaveLength(3);
  });

  it("offers none where the history cannot answer what the alert reads", () => {
    const named = { ...NEW_ALERT.filters, agent: "support-bot" };
    for (const over of [
      { filters: { ...named, model: "gpt-4o" } },
      { filters: { ...NEW_ALERT.filters, agent: "a, b" } },
      { baseline: RuleBaseline.Previous },
      { measure: RuleMeasure.Tokens, aggregation: RuleAggregation.Sum, window: RuleWindow.Day },
      { measure: RuleMeasure.Failures, aggregation: RuleAggregation.Avg, window: RuleWindow.Day },
      { measure: RuleMeasure.Failures, aggregation: RuleAggregation.Count },
      { filters: { ...NEW_ALERT.filters, agent: "unknown-bot" } },
    ] satisfies Partial<AlertDraft>[]) {
      expect(suggestionFor(draft(over), HISTORY)).toBeNull();
    }
    expect(suggestionFor(draft({}), null)).toBeNull();
  });
});
