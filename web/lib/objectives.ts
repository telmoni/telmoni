// An objective's form and words: one agent's runs promised to succeed, or to
// end inside a latency, over 7, 28 or 30 days, kept as the burn alert it is
// (`crates/telemetry/src/rules/mod.rs`, `Objective`), and checked here as the
// lane checks it so a mistake is said beside its field.

import {
  ALERT_NAME_MAX,
  linksOf,
  numberOf,
  renotifyOf,
  splitValues,
  type LinkDraft,
  type RuleInput,
} from "@/lib/alerts";
import type { Alert, History, Percentiles } from "@/lib/server/data";
import {
  MissingData,
  RuleAggregation,
  RuleBaseline,
  RuleMeasure,
  RuleOperator,
  RuleWindow,
  SpanKind,
} from "@/lib/types/enums";

/** Where the Objectives page's focus lands once an objective's row is gone. */
export const OBJECTIVES_HEADING = "objectives-heading";

/** The days an objective's budget is counted over, as the lane takes them. */
export const OBJECTIVE_DAYS = [30, 28, 7] as const;

/** The burns that page and warn, as the lane fixes them: 2% of a 30 days'
 *  budget in an hour, and 5% in six hours. */
export const FAST_BURN = 14.4;
export const SLOW_BURN = 6;

/** The share of runs an objective may promise, in percent. */
const TARGET_MIN = 50;
const TARGET_MAX = 99.99;

/** The longest a latency objective's runs are given, in milliseconds. */
const LATENCY_MAX_MS = 3_600_000;

/** What an objective promises of its runs. */
export type Promised = "success" | "latency";

/** The form's state, every value as typed. */
export type ObjectiveDraft = {
  name: string;
  agent: string;
  promise: Promised;
  target: string;
  latencyMs: string;
  days: (typeof OBJECTIVE_DAYS)[number];
  renotifyMinutes: string;
  connectors: LinkDraft[];
};

export const NEW_OBJECTIVE: ObjectiveDraft = {
  name: "",
  agent: "",
  promise: "success",
  target: "99",
  latencyMs: "",
  days: 30,
  renotifyMinutes: "",
  connectors: [],
};

/** The form for an objective saved before. */
export function objectiveDraftOf(alert: Alert): ObjectiveDraft {
  const objective = alert.objective;
  const days = OBJECTIVE_DAYS.find((d) => d === objective?.days) ?? 30;
  return {
    name: alert.name,
    agent: alert.filters.agent?.[0] ?? "",
    promise: objective?.latency_ms == null ? "success" : "latency",
    target: objective ? String(Number((objective.target * 100).toPrecision(6))) : NEW_OBJECTIVE.target,
    latencyMs: objective?.latency_ms == null ? "" : String(objective.latency_ms),
    days,
    renotifyMinutes: alert.renotify_minutes === null ? "" : String(alert.renotify_minutes),
    connectors: alert.connectors.map((link) => ({
      connectionId: link.connection_id,
      level: link.level,
      hours: link.hours,
    })),
  };
}

/** The form checked into the burn alert the lane keeps, or what is wrong with it. */
export function objectiveInputOf(
  draft: ObjectiveDraft,
): { ok: true; input: RuleInput } | { ok: false; error: string } {
  const fail = (error: string) => ({ ok: false as const, error });
  const name = draft.name.trim();
  if (!name || [...name].length > ALERT_NAME_MAX) {
    return fail(`Name the objective: 1 to ${ALERT_NAME_MAX} characters.`);
  }
  const agents = splitValues(draft.agent);
  if (agents.length !== 1) return fail("Name the one agent whose runs it promises.");
  const target = numberOf(draft.target);
  if (target === null || target < TARGET_MIN || target > TARGET_MAX) {
    return fail(`Promise ${TARGET_MIN}% to ${TARGET_MAX}% of its runs.`);
  }
  let latency: number | null = null;
  if (draft.promise === "latency") {
    latency = numberOf(draft.latencyMs);
    if (latency === null || !Number.isInteger(latency) || latency < 1 || latency > LATENCY_MAX_MS) {
      return fail(`Its runs end inside 1 to ${LATENCY_MAX_MS.toLocaleString("en-US")} ms, a whole number.`);
    }
  }
  const again = renotifyOf(draft.renotifyMinutes);
  if (!again.ok) return again;
  const links = linksOf(draft.connectors);
  if (!links.ok) return links;
  return {
    ok: true,
    input: {
      name,
      kind: SpanKind.Run,
      measure: RuleMeasure.Burn,
      aggregation: RuleAggregation.Avg,
      filters: { agent: agents },
      window: RuleWindow.Hour,
      operator: RuleOperator.Gte,
      alert_threshold: FAST_BURN,
      warning_threshold: SLOW_BURN,
      missing_data: MissingData.Zero,
      renotify_minutes: again.minutes,
      score_name: null,
      baseline: RuleBaseline.None,
      // Without the float noise a percentage leaves (99.9 / 100 is 0.9990000000000001).
      objective: { target: Number((target / 100).toPrecision(12)), latency_ms: latency, days: draft.days },
      connectors: links.connectors,
    },
  };
}

/** The agents `history` holds runs of, by name. */
export function agentsOf(history: History | null): string[] {
  return (history?.agents ?? [])
    .filter((row) => row.agent !== null && row.kind === SpanKind.Run)
    .map((row) => row.agent as string)
    .toSorted((a, b) => a.localeCompare(b));
}

/** The agent's runs' durations, which a latency may start from; none where
 *  its history holds no run. */
export function runDurationsOf(history: History | null, agent: string): Percentiles | null {
  const name = agent.trim();
  return history?.agents.find((row) => row.agent === name && row.kind === SpanKind.Run)?.duration_ms ?? null;
}

/** How long a budget lasts at `burn`, in words: none where nothing burns. */
export function lastsWords(days: number, burn: number): string | null {
  if (!(burn > 0)) return null;
  const left = days / burn;
  if (left < 1) return `${Math.max(1, Math.round(left * 24))} hours`;
  return left < 10 ? `${left.toFixed(1)} days` : `${Math.round(left)} days`;
}
