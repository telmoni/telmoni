// The alerts page's words and checks: an alert's metric read as its notices
// read it (`crates/telemetry/src/notify.rs`), its thresholds in the unit it
// measures, and the form's draft turned into what the lane takes, checked as
// the lane checks it (`crates/telemetry/src/rules/mod.rs`) so a mistake is
// said beside the field before anything is sent.

import { connectionName } from "@/lib/connect";
import { isOneOf } from "@/lib/one-of";
import type {
  Alert,
  Connection,
  Hours,
  Incident,
  Mute,
  RuleFilters,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import { formatCost } from "@/lib/traces";
import { UTC_OFFSET_EAST, UTC_OFFSET_WEST } from "@/lib/utc-offset";
import {
  type ConnectionStatus,
  IncidentStatus,
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
} from "@/lib/types/enums";

/** The kinds of span an alert reads: every kind but the bare span. */
export const ALERT_KINDS = [SpanKind.Run, SpanKind.Step, SpanKind.ModelCall, SpanKind.ToolCall] as const;
export type AlertKind = (typeof ALERT_KINDS)[number];

export function isAlertKind(kind: SpanKind): kind is AlertKind {
  return isOneOf(ALERT_KINDS, kind);
}

export const KIND_PLURAL: Record<AlertKind, string> = {
  [SpanKind.Run]: "runs",
  [SpanKind.Step]: "steps",
  [SpanKind.ModelCall]: "model calls",
  [SpanKind.ToolCall]: "tool calls",
};

/** The longest name an alert takes, the table's CHECK. */
export const ALERT_NAME_MAX = 60;

/** The longest re-notification period, in minutes: a week. */
export const RENOTIFY_MAX_MINUTES = 10_080;

/** The most connectors one alert links. */
export const LINKS_MAX = 20;

/** The most incidents the list answers: a full list may leave out the oldest
 *  resolved ones. */
export const INCIDENTS_MAX = 200;

/** What an alert of `kind` measures: tokens on model calls alone, and cost on
 *  runs alone, from their calls. */
export function measuresFor(kind: AlertKind): RuleMeasure[] {
  return [
    RuleMeasure.Count,
    RuleMeasure.Failures,
    RuleMeasure.Duration,
    ...(kind === SpanKind.ModelCall ? [RuleMeasure.Tokens] : []),
    ...(kind === SpanKind.Run ? [RuleMeasure.Cost] : []),
  ];
}

/** How `measure` is folded over the window, as the server's `folds` admits:
 *  what the hourly totals can answer. */
export function aggregationsFor(measure: RuleMeasure): RuleAggregation[] {
  switch (measure) {
    case RuleMeasure.Count:
      return [RuleAggregation.Count];
    case RuleMeasure.Failures:
      return [RuleAggregation.Avg, RuleAggregation.Count];
    case RuleMeasure.Duration:
      return [
        RuleAggregation.P95,
        RuleAggregation.P99,
        RuleAggregation.P90,
        RuleAggregation.P50,
        RuleAggregation.Avg,
        RuleAggregation.Sum,
      ];
    case RuleMeasure.Tokens:
      return [RuleAggregation.Avg, RuleAggregation.Sum];
    case RuleMeasure.Cost:
      return [RuleAggregation.Max, RuleAggregation.Avg, RuleAggregation.Sum];
  }
}

export const MEASURE_LABEL: Record<RuleMeasure, string> = {
  [RuleMeasure.Count]: "Count",
  [RuleMeasure.Failures]: "Failures",
  [RuleMeasure.Duration]: "Duration",
  [RuleMeasure.Tokens]: "Tokens",
  [RuleMeasure.Cost]: "Cost",
};

/** How an aggregation of `measure` reads in the form. */
export function aggregationLabel(measure: RuleMeasure, aggregation: RuleAggregation): string {
  switch (measure) {
    case RuleMeasure.Count:
      return "How many";
    case RuleMeasure.Failures:
      return aggregation === RuleAggregation.Avg ? "Share failed" : "How many failed";
    case RuleMeasure.Cost:
      return aggregation === RuleAggregation.Max
        ? "Most a run spent (a cost cap)"
        : aggregation === RuleAggregation.Avg
          ? "Average a run spent"
          : "What the runs spent";
    default:
      return aggregation === RuleAggregation.Avg
        ? "Average"
        : aggregation === RuleAggregation.Sum
          ? "Total"
          : aggregation;
  }
}

export const WINDOW_LABEL: Record<RuleWindow, string> = {
  [RuleWindow.Hour]: "Last hour",
  [RuleWindow.Day]: "Last day",
  [RuleWindow.Week]: "Last week",
};

type Metric = Pick<Alert, "kind" | "measure" | "aggregation" | "window">;

/** An alert's metric in its notices' own words: "The p95 duration of model
 *  calls over the last hour", "The most a run spent over the last day". */
export function metricWords(alert: Metric): string {
  const over = `over the last ${alert.window}`;
  if (alert.measure === RuleMeasure.Cost) {
    const spent =
      alert.aggregation === RuleAggregation.Max
        ? "The most a run spent"
        : alert.aggregation === RuleAggregation.Avg
          ? "The average a run spent"
          : "What the runs spent";
    return `${spent} ${over}`;
  }
  const folded = (() => {
    switch (alert.measure) {
      case RuleMeasure.Count:
        return "The count";
      case RuleMeasure.Failures:
        return alert.aggregation === RuleAggregation.Avg ? "The share failed" : "The failures";
      case RuleMeasure.Duration:
        return alert.aggregation === RuleAggregation.Avg
          ? "The average duration"
          : alert.aggregation === RuleAggregation.Sum
            ? "The summed duration"
            : `The ${alert.aggregation} duration`;
      default:
        return alert.aggregation === RuleAggregation.Avg ? "The average tokens" : "The tokens";
    }
  })();
  const of = isAlertKind(alert.kind) ? KIND_PLURAL[alert.kind] : "spans";
  return `${folded} of ${of} ${over}`;
}

/** The unit a value is typed and read in: milliseconds for a duration, a
 *  percentage for the share failed, US dollars for a cost, else a number. */
export type Unit = "ms" | "percent" | "dollars" | "number";

export function unitOf(measure: RuleMeasure, aggregation: RuleAggregation): Unit {
  if (measure === RuleMeasure.Duration) return "ms";
  if (measure === RuleMeasure.Cost) return "dollars";
  if (measure === RuleMeasure.Failures && aggregation === RuleAggregation.Avg) return "percent";
  return "number";
}

export const UNIT_SUFFIX: Record<Unit, string> = { ms: "ms", percent: "%", dollars: "USD", number: "" };

const NUMBER = new Intl.NumberFormat("en-US", { maximumFractionDigits: 2 });

/** A value in its unit, as the page reads it: `6,200 ms`, `25%`, `$3.45`. */
export function amount(measure: RuleMeasure, aggregation: RuleAggregation, value: number): string {
  switch (unitOf(measure, aggregation)) {
    case "ms":
      return `${NUMBER.format(value)} ms`;
    case "percent":
      return `${NUMBER.format(value * 100)}%`;
    case "dollars":
      return formatCost(value);
    default:
      return NUMBER.format(value);
  }
}

export const OPERATOR_SYMBOL: Record<RuleOperator, string> = {
  [RuleOperator.Gt]: ">",
  [RuleOperator.Gte]: "≥",
  [RuleOperator.Lt]: "<",
  [RuleOperator.Lte]: "≤",
  [RuleOperator.Eq]: "=",
  [RuleOperator.Ne]: "≠",
};

export const OPERATOR_LABEL: Record<RuleOperator, string> = {
  [RuleOperator.Gt]: "Above (>)",
  [RuleOperator.Gte]: "At or above (≥)",
  [RuleOperator.Lt]: "Below (<)",
  [RuleOperator.Lte]: "At or below (≤)",
  [RuleOperator.Eq]: "Equal to (=)",
  [RuleOperator.Ne]: "Not equal to (≠)",
};

/** An alert's thresholds in words: `> 5,000 ms`, and its warning's. */
export function thresholdWords(
  alert: Pick<Alert, "measure" | "aggregation" | "operator" | "alert_threshold" | "warning_threshold">,
): { alert: string; warning: string | null } {
  const at = (value: number) =>
    `${OPERATOR_SYMBOL[alert.operator]} ${amount(alert.measure, alert.aggregation, value)}`;
  return {
    alert: at(alert.alert_threshold),
    warning: alert.warning_threshold === null ? null : at(alert.warning_threshold),
  };
}

/** Whether `operator` admits a warning: an equality has no way toward the
 *  alert to meet one first. */
export function takesWarning(operator: RuleOperator): boolean {
  return operator !== RuleOperator.Eq && operator !== RuleOperator.Ne;
}

export const MISSING_DATA_LABEL: Record<MissingData, string> = {
  [MissingData.Zero]: "Read as 0",
  [MissingData.Keep]: "Keeps its last severity",
  [MissingData.NoData]: "Shown as no data",
  [MissingData.NotifyNoData]: "Told once a whole window has none",
};

export const SEVERITY_LABEL: Record<Severity, string> = {
  [Severity.Unknown]: "Not evaluated",
  [Severity.Ok]: "OK",
  [Severity.Warning]: "Warning",
  [Severity.Alert]: "Alert",
  [Severity.NoData]: "No data",
  [Severity.Paused]: "Paused",
};

/** A severity's badge: an alert in the error's colour, a warning filled, a
 *  pause or no data muted, the rest outlined. */
export function severityVariant(
  severity: Severity,
): "destructive" | "default" | "secondary" | "outline" {
  switch (severity) {
    case Severity.Alert:
      return "destructive";
    case Severity.Warning:
      return "default";
    case Severity.NoData:
    case Severity.Paused:
      return "secondary";
    default:
      return "outline";
  }
}

export const STATUS_LABEL: Record<IncidentStatus, string> = {
  [IncidentStatus.Open]: "Open",
  [IncidentStatus.Acknowledged]: "Acknowledged",
  [IncidentStatus.Resolved]: "Resolved",
};

export const LEVEL_LABEL: Record<RuleLevel, string> = {
  [RuleLevel.Warning]: "Warnings and alerts",
  [RuleLevel.Alert]: "Alerts only",
};

// ── Filters ──────────────────────────────────────────────────────────────────

/** The text fields an alert filters by, each any of its values. */
export const FILTER_FIELDS = ["agent", "model", "provider", "tool", "environment", "release"] as const;
export type FilterField = (typeof FILTER_FIELDS)[number];

export const FILTER_LABEL: Record<FilterField, string> = {
  agent: "Agent",
  model: "Model",
  provider: "Provider",
  tool: "Tool",
  environment: "Environment",
  release: "Release",
};

/** The fields an alert's spans carry, so the ones its filters can name: a
 *  model call's model and provider, a tool call's tool, and a run's cost by
 *  its calls' model and provider. */
export function filterFieldsFor(kind: AlertKind, measure: RuleMeasure): FilterField[] {
  const own: FilterField[] =
    measure === RuleMeasure.Cost || kind === SpanKind.ModelCall
      ? ["model", "provider"]
      : kind === SpanKind.ToolCall
        ? ["tool"]
        : [];
  return ["agent", ...own, "environment", "release"];
}

/** Whether an alert filters by status: a run's cost does not, since it is
 *  read from the calls as they land, before the run's own status is known. */
export function filtersStatus(measure: RuleMeasure): boolean {
  return measure !== RuleMeasure.Cost;
}

export const FILTER_STATUSES = [SpanStatus.Ok, SpanStatus.Error, SpanStatus.Cancelled] as const;

/** An alert's filters in words, field by field: `model claude-sonnet-5-5`. */
export function filterWords(filters: RuleFilters): string[] {
  const words: string[] = [];
  for (const field of FILTER_FIELDS) {
    const values = filters[field] ?? [];
    if (values.length > 0) words.push(`${FILTER_LABEL[field].toLowerCase()} ${joinValues(values)}`);
  }
  if (filters.status && filters.status.length > 0) words.push(`status ${filters.status.join(", ")}`);
  return words;
}

/** Values typed into one field, comma-separated: each trimmed, an empty one
 *  dropped and one given twice kept once. A value that holds a comma is
 *  typed between double quotes, a quote in it doubled, as [`joinValues`]
 *  writes it. */
export function splitValues(text: string): string[] {
  const kept: string[] = [];
  const keep = (piece: string) => {
    const value = piece.trim();
    if (value && !kept.includes(value)) kept.push(value);
  };
  let piece = "";
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') {
        piece += '"';
        i++;
      } else if (c === '"') {
        quoted = false;
      } else {
        piece += c;
      }
    } else if (c === '"' && piece.trim() === "") {
      piece = "";
      quoted = true;
    } else if (c === ",") {
      keep(piece);
      piece = "";
    } else {
      piece += c;
    }
  }
  keep(piece);
  return kept;
}

/** Values as one field types them: comma-separated, one holding a comma or
 *  opening with a quote between double quotes, so [`splitValues`] reads them
 *  back as they were. */
export function joinValues(values: readonly string[]): string {
  return values
    .map((value) => (/[,"]/.test(value) ? `"${value.replace(/"/g, '""')}"` : value))
    .join(", ");
}

// ── Hours ────────────────────────────────────────────────────────────────────

const DAY_SHORT = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] as const;

/** Days 1 (Monday) to 7, as the form's toggles name them. */
export const DAYS = DAY_SHORT.map((label, i) => ({ day: i + 1, label }));

/** An offset from UTC in minutes east, as a clock reads it: `UTC+01:00`,
 *  `UTC−05:30`, and `UTC` at none. */
export function offsetWords(minutes: number): string {
  if (minutes === 0) return "UTC";
  const sign = minutes > 0 ? "+" : "−";
  const whole = Math.abs(minutes);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `UTC${sign}${pad(Math.floor(whole / 60))}:${pad(whole % 60)}`;
}

/** Every offset the lane admits, a quarter-hour apart: -12:00 to +14:00. */
export const OFFSETS: number[] = Array.from(
  { length: (UTC_OFFSET_EAST - UTC_OFFSET_WEST) / 15 + 1 },
  (_, i) => UTC_OFFSET_WEST + i * 15,
);

/** The browser's own offset from UTC now, in minutes east. */
export function browserOffset(): number {
  return -new Date().getTimezoneOffset() || 0;
}

/** Hours in words: `Mon–Fri 09:00–18:00 UTC+01:00`, a day's whole span as
 *  `all day`, and a run of days joined by a dash. */
export function hoursWords(hours: Hours): string {
  const days = [...new Set(hours.days)].filter((d) => d >= 1 && d <= 7).sort((a, b) => a - b);
  const runs: string[] = [];
  for (let i = 0; i < days.length; ) {
    let j = i;
    while (j + 1 < days.length && days[j + 1] === days[j] + 1) j++;
    const [first, last] = [DAY_SHORT[days[i] - 1], DAY_SHORT[days[j] - 1]];
    if (j - i >= 2) runs.push(`${first}–${last}`);
    else runs.push(...days.slice(i, j + 1).map((d) => DAY_SHORT[d - 1]));
    i = j + 1;
  }
  const when = days.length === 7 ? "Every day" : runs.join(", ");
  const span = hours.from === hours.to ? "all day" : `${hours.from}–${hours.to}`;
  return `${when} ${span} ${offsetWords(hours.utc_offset)}`;
}

const TIME = /^([01]\d|2[0-3]):[0-5]\d$/;

// ── The form ─────────────────────────────────────────────────────────────────

/** One of the project's connections, as the form offers it, with the kinds
 *  of notice it takes: none for every kind, as a webhook's events narrow them. */
export type ConnectionChoice = {
  id: string;
  name: string;
  status: ConnectionStatus;
  eventKinds: Connection["event_kinds"];
};

/** A connection's state where it hears nothing, as the Connectors page says
 *  it: a delivery goes to an active one alone. */
export const CONNECTION_STATUS_LABEL: Record<ConnectionChoice["status"], string | null> = {
  active: null,
  errored: "Needs attention",
  revoked: "Disconnected",
};

/** What a connection takes of an alert's incidents, where not both kinds:
 *  notifications sends a webhook only the kinds its events name. */
export function incidentKindsLabel(eventKinds: ConnectionChoice["eventKinds"]): string | null {
  if (eventKinds === null) return null;
  const opened = eventKinds.includes(NotificationKind.IncidentOpened);
  const resolved = eventKinds.includes(NotificationKind.IncidentResolved);
  if (opened && resolved) return null;
  if (opened) return "Takes openings only";
  if (resolved) return "Takes resolutions only";
  return "Takes no incidents";
}

/** The project's connections, each by a name that tells it from the rest: a
 *  webhook on a host another shares, with the start of its fingerprint, as
 *  the Connectors page tells them apart. */
export function connectionChoices(connections: readonly Connection[]): ConnectionChoice[] {
  const names = connections.map(connectionName);
  return connections.map((connection, i) => ({
    id: connection.id,
    name:
      connection.provider === "webhook" && names.filter((name) => name === names[i]).length > 1
        ? `${names[i]} (${connection.channel_id.slice(0, 8)})`
        : names[i],
    status: connection.status,
    eventKinds: connection.event_kinds,
  }));
}

/** A connector as the form links it. */
export type LinkDraft = { connectionId: string; level: RuleLevel; hours: Hours | null };

/** The form's state: every value as typed, checked into a [`RuleInput`] on
 *  submit by [`inputOf`]. */
export type AlertDraft = {
  name: string;
  kind: AlertKind;
  measure: RuleMeasure;
  aggregation: RuleAggregation;
  window: RuleWindow;
  operator: RuleOperator;
  alertThreshold: string;
  warningThreshold: string;
  missingData: MissingData;
  renotifyMinutes: string;
  filters: Record<FilterField, string>;
  statuses: SpanStatus[];
  connectors: LinkDraft[];
};

/** An alert as the lane takes it, new or saved over. */
export type RuleInput = {
  name: string;
  kind: AlertKind;
  measure: RuleMeasure;
  aggregation: RuleAggregation;
  filters: RuleFilters;
  window: RuleWindow;
  operator: RuleOperator;
  alert_threshold: number;
  warning_threshold: number | null;
  missing_data: MissingData;
  renotify_minutes: number | null;
  connectors: { connection_id: string; level: RuleLevel; hours: Hours | null }[];
};

const NO_FILTERS: Record<FilterField, string> = {
  agent: "",
  model: "",
  provider: "",
  tool: "",
  environment: "",
  release: "",
};

/** A new alert's form: the share of runs that failed over the last hour,
 *  its thresholds the person's to type. */
export const NEW_ALERT: AlertDraft = {
  name: "",
  kind: SpanKind.Run,
  measure: RuleMeasure.Failures,
  aggregation: RuleAggregation.Avg,
  window: RuleWindow.Hour,
  operator: RuleOperator.Gt,
  alertThreshold: "",
  warningThreshold: "",
  missingData: MissingData.Zero,
  renotifyMinutes: "",
  filters: NO_FILTERS,
  statuses: [],
  connectors: [],
};

/** A number as the form shows it: in the unit it is typed in, without the
 *  float noise a percentage's sum leaves (`10`, not `10.000000000000002`). */
function shown(value: number, unit: Unit): string {
  const typed = unit === "percent" ? value * 100 : value;
  return String(Number(typed.toPrecision(12)));
}

/** The form for an alert saved before: its values as the form types them. */
export function draftOf(alert: Alert): AlertDraft {
  const unit = unitOf(alert.measure, alert.aggregation);
  const filters = { ...NO_FILTERS };
  for (const field of FILTER_FIELDS) filters[field] = joinValues(alert.filters[field] ?? []);
  return {
    name: alert.name,
    kind: isAlertKind(alert.kind) ? alert.kind : SpanKind.Run,
    measure: alert.measure,
    aggregation: alert.aggregation,
    window: alert.window,
    operator: alert.operator,
    alertThreshold: shown(alert.alert_threshold, unit),
    warningThreshold: alert.warning_threshold === null ? "" : shown(alert.warning_threshold, unit),
    missingData: alert.missing_data,
    renotifyMinutes: alert.renotify_minutes === null ? "" : String(alert.renotify_minutes),
    filters,
    statuses: alert.filters.status ?? [],
    connectors: alert.connectors.map((link) => ({
      connectionId: link.connection_id,
      level: link.level,
      hours: link.hours,
    })),
  };
}

/** The form with its kind changed: a measure it no longer offers falls back
 *  to the count, and an aggregation the measure does not fold to its first. */
export function withKind(draft: AlertDraft, kind: AlertKind): AlertDraft {
  const measure = measuresFor(kind).includes(draft.measure) ? draft.measure : RuleMeasure.Count;
  return withMeasure({ ...draft, kind }, measure);
}

/** The form with its measure changed, its aggregation kept where it folds. */
export function withMeasure(draft: AlertDraft, measure: RuleMeasure): AlertDraft {
  const folds = aggregationsFor(measure);
  const aggregation = folds.includes(draft.aggregation) ? draft.aggregation : folds[0];
  return { ...draft, measure, aggregation };
}

/** A comma that groups thousands, which a number may carry; any other is a
 *  decimal comma, read as nothing rather than as a number ten times over. */
const GROUPED = /^-?\d{1,3}(,\d{3})+(\.\d+)?$/;

/** A typed number, in the form's unit, as the lane's: grouping commas
 *  allowed, a percentage a share. None for anything but a finite number. */
function parsed(text: string, unit: Unit): number | null {
  const trimmed = text.trim();
  if (!trimmed || (trimmed.includes(",") && !GROUPED.test(trimmed))) return null;
  const value = Number(trimmed.replace(/,/g, ""));
  if (!Number.isFinite(value)) return null;
  return unit === "percent" ? value / 100 : value;
}

/** The form checked into what the lane takes, or what is wrong with it, in
 *  the words a person fixes it by. */
export function inputOf(
  draft: AlertDraft,
): { ok: true; input: RuleInput } | { ok: false; error: string } {
  const fail = (error: string) => ({ ok: false as const, error });
  const name = draft.name.trim();
  if (!name || [...name].length > ALERT_NAME_MAX) {
    return fail(`Name the alert: 1 to ${ALERT_NAME_MAX} characters.`);
  }
  if (!measuresFor(draft.kind).includes(draft.measure)) {
    return fail(`${MEASURE_LABEL[draft.measure]} is not measured on ${KIND_PLURAL[draft.kind]}.`);
  }
  if (!aggregationsFor(draft.measure).includes(draft.aggregation)) {
    return fail("Choose how the measure is folded over the window.");
  }
  const unit = unitOf(draft.measure, draft.aggregation);
  const alert = parsed(draft.alertThreshold, unit);
  if (alert === null) return fail("The alert threshold is a number, its decimals after a point.");
  let warning: number | null = null;
  if (draft.warningThreshold.trim() && takesWarning(draft.operator)) {
    warning = parsed(draft.warningThreshold, unit);
    if (warning === null) {
      return fail("The warning threshold is a number, its decimals after a point, or left empty.");
    }
    const below = draft.operator === RuleOperator.Gt || draft.operator === RuleOperator.Gte;
    if (below ? warning >= alert : warning <= alert) {
      return fail(
        below
          ? "The warning threshold is met first: below the alert threshold."
          : "The warning threshold is met first: above the alert threshold.",
      );
    }
  }
  let renotify: number | null = null;
  if (draft.renotifyMinutes.trim()) {
    renotify = parsed(draft.renotifyMinutes, "number");
    if (renotify === null || !Number.isInteger(renotify) || renotify < 1 || renotify > RENOTIFY_MAX_MINUTES) {
      return fail(`Notify again every 1 to ${RENOTIFY_MAX_MINUTES.toLocaleString("en-US")} minutes, or leave it empty.`);
    }
  }
  if (draft.connectors.length > LINKS_MAX) return fail(`An alert links up to ${LINKS_MAX} connectors.`);
  const seen = new Set<string>();
  for (const link of draft.connectors) {
    if (seen.has(link.connectionId)) return fail("An alert links a connector once.");
    seen.add(link.connectionId);
    const hours = link.hours;
    if (hours) {
      if (hours.days.length === 0) return fail("Set hours name a day at least.");
      if (!TIME.test(hours.from) || !TIME.test(hours.to)) return fail("Set hours run from and to a time of day.");
    }
  }
  const filters: RuleFilters = {};
  for (const field of filterFieldsFor(draft.kind, draft.measure)) {
    const values = splitValues(draft.filters[field]);
    if (values.length > 0) filters[field] = values;
  }
  if (filtersStatus(draft.measure) && draft.statuses.length > 0) {
    filters.status = FILTER_STATUSES.filter((status) => draft.statuses.includes(status));
  }
  return {
    ok: true,
    input: {
      name,
      kind: draft.kind,
      measure: draft.measure,
      aggregation: draft.aggregation,
      filters,
      window: draft.window,
      operator: draft.operator,
      alert_threshold: alert,
      warning_threshold: warning,
      missing_data: draft.missingData,
      renotify_minutes: renotify,
      connectors: draft.connectors.map((link) => ({
        connection_id: link.connectionId,
        level: link.level,
        hours: link.hours
          ? { ...link.hours, days: [...new Set(link.hours.days)].sort((a, b) => a - b) }
          : null,
      })),
    },
  };
}

/** How often a held severity is told again, in words. */
export function renotifyWords(minutes: number | null): string {
  if (minutes === null) return "Never";
  if (minutes % 1_440 === 0) return minutes === 1_440 ? "Every day" : `Every ${minutes / 1_440} days`;
  if (minutes % 60 === 0) return minutes === 60 ? "Every hour" : `Every ${minutes / 60} hours`;
  return minutes === 1 ? "Every minute" : `Every ${minutes} minutes`;
}

// ── Maintenance windows ──────────────────────────────────────────────────────

/** The longest a maintenance window runs, the lane's thirty days. */
export const MUTE_MINUTES_MAX = 30 * 1_440;

/** The lengths a window is muted for, the lane's thirty days the longest. */
export const MUTE_LENGTHS = [
  { label: "30 minutes", minutes: 30 },
  { label: "1 hour", minutes: 60 },
  { label: "2 hours", minutes: 120 },
  { label: "4 hours", minutes: 240 },
  { label: "8 hours", minutes: 480 },
  { label: "1 day", minutes: 1_440 },
  { label: "3 days", minutes: 4_320 },
  { label: "1 week", minutes: 10_080 },
  { label: "30 days", minutes: 43_200 },
] as const;

/** The windows not yet over that mute `ruleId`, its own and the project's,
 *  the soonest to end first; with no rule, the project's alone. One a
 *  member ended is over, even one still to start, which the lane ends a
 *  moment after its start. */
export function windowsFor(mutes: Mute[], ruleId: string | null, now: number): Mute[] {
  return mutes
    .filter((mute) => mute.ended_by === null && Date.parse(mute.ends_at) > now)
    .filter((mute) => mute.rule_id === null || mute.rule_id === ruleId)
    .sort((a, b) => Date.parse(a.ends_at) - Date.parse(b.ends_at));
}

/** Whether a window mutes now or is still to start. */
export function inForce(mute: Mute, now: number): boolean {
  return Date.parse(mute.starts_at) <= now && Date.parse(mute.ends_at) > now;
}

// ── Incidents ────────────────────────────────────────────────────────────────

/** The Alerts page's heading, where the focus lands once an alert, or the
 *  window over every alert, goes. */
export const ALERTS_HEADING = "alerts-heading";

/** An alert's heading, where the focus lands once a control in its card goes. */
export function alertHeading(ruleId: string): string {
  return `alert-${ruleId}-name`;
}

/** The alerts page at one incident, under its alert, which it draws even past
 *  its list, and scrolls to. */
export function incidentHref(organization: string, project: string, incidentId: string): string {
  const id = encodeURIComponent(incidentId);
  return projectPath(organization, project, `/alerts?incident=${id}#incident-${id}`);
}

/** One entry of a roster, the organization's or the project's. */
type Named = { member_id: string; display_name?: string | null; email: string };

/** Who a member's id names, from the rosters that could be read: the
 *  organization's, which its owner and admins alone read, then the project's.
 *  Someone neither names has left, where the organization's was read; else
 *  they may be an admin of it, seated on no project. */
export function memberNames(
  organization: readonly Named[] | null,
  project: readonly Named[] | null,
): (id: string) => string {
  const people = new Map(
    [...(organization ?? []), ...(project ?? [])].map((member) => [
      member.member_id,
      member.display_name || member.email,
    ]),
  );
  const unknown = organization
    ? "a former member"
    : project
      ? "an organization admin or a former member"
      : "someone";
  return (id) => people.get(id) ?? unknown;
}

/** Where an incident's notices went, as a sentence lists it: the feed,
 *  always, then each connection by its name, one `names` lacks by `unknown`. */
export function toldWords(
  incident: Pick<Incident, "told_to">,
  names: ReadonlyMap<string, string>,
  unknown: string,
): string[] {
  return ["the feed", ...incident.told_to.map((id) => names.get(id) ?? unknown)];
}
