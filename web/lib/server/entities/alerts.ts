import { cache } from "react";
import { z } from "zod";

import {
  IncidentStatus,
  MissingData,
  MonitorCause,
  PausedBy,
  RuleAggregation,
  RuleBaseline,
  RuleLevel,
  RuleMeasure,
  RuleOperator,
  RuleWindow,
  Severity,
  SpanKind,
  SpanStatus,
} from "@/lib/types/enums";
import { TraceIdSchema } from "@/lib/types/traces";

import { ChangeSchema } from "./changes";
import { MISSING, readLane } from "./traces";

// The hours a linked connector hears an alert in: days 1 (Monday) to 7, from
// and to as HH:MM, at a fixed offset from UTC in minutes.
const HoursSchema = z.object({
  days: z.array(z.number().int()),
  from: z.string(),
  to: z.string(),
  utc_offset: z.number().int(),
});
export type Hours = z.infer<typeof HoursSchema>;

// A connector an alert links, and whether five failed deliveries in a row
// muted it for this alert.
const AlertLinkSchema = z.object({
  connection_id: z.string(),
  level: z.enum(RuleLevel),
  hours: HoursSchema.nullable(),
  muted: z.boolean(),
});

// The fields an alert's spans are filtered by, each any of its values; a
// field it does not filter by is absent.
const RuleFiltersSchema = z.object({
  agent: z.array(z.string()).optional(),
  model: z.array(z.string()).optional(),
  provider: z.array(z.string()).optional(),
  tool: z.array(z.string()).optional(),
  source: z.array(z.string()).optional(),
  status: z.array(z.enum(SpanStatus)).optional(),
  environment: z.array(z.string()).optional(),
  release: z.array(z.string()).optional(),
  tag: z.array(z.string()).optional(),
});
export type RuleFilters = z.infer<typeof RuleFiltersSchema>;

// An objective's promise: `target` of one agent's traces succeed, or end
// inside `latency_ms`, over its days.
const ObjectiveSchema = z.object({
  target: z.number(),
  latency_ms: z.number().int().nullable(),
  days: z.number().int(),
});
export type Objective = z.infer<typeof ObjectiveSchema>;

// One alert: its definition, who paused it, and its last evaluation.
const AlertSchema = z.object({
  id: z.string(),
  name: z.string(),
  kind: z.enum(SpanKind),
  measure: z.enum(RuleMeasure),
  aggregation: z.enum(RuleAggregation),
  filters: RuleFiltersSchema,
  window: z.enum(RuleWindow),
  operator: z.enum(RuleOperator),
  alert_threshold: z.number(),
  warning_threshold: z.number().nullable(),
  missing_data: z.enum(MissingData),
  renotify_minutes: z.number().int().nullable(),
  /** A score alert's score, by its name; none for any other. */
  score_name: z.string().nullable(),
  /** Read as it is, or as its change in percent from the window before. */
  baseline: z.enum(RuleBaseline),
  /** A burn alert's objective; absent for any other. */
  objective: ObjectiveSchema.optional(),
  /** A member, by hand, or its plan, past the organization's number. */
  paused: z.enum(PausedBy).nullable(),
  paused_at: z.string().nullable(),
  /** None before its first evaluation, or where its window held no span. */
  value: z.number().nullable(),
  evaluated_at: z.string().nullable(),
  severity: z.enum(Severity),
  severity_since: z.string(),
  connectors: z.array(AlertLinkSchema),
  /** Made by the project's first trace rather than a member. */
  starter: z.boolean(),
  created_at: z.string(),
  updated_at: z.string(),
});
export type Alert = z.infer<typeof AlertSchema>;

// One incident: its alert or its monitor, where it stands, who took or
// resolved it, where its notices went — the connections, the feed apart — and
// the traces they showed, oldest first. A monitor's names its agent and cause.
// Its changes are what the agents it watches changed to in the day before it
// opened, the newest first: none where they could not be read.
export const IncidentSchema = z.object({
  id: z.string(),
  rule_id: z.string().nullable(),
  rule_name: z.string().nullable(),
  monitor_id: z.string().nullable(),
  agent: z.string().nullable(),
  cause: z.enum(MonitorCause).nullable(),
  status: z.enum(IncidentStatus),
  severity: z.enum(Severity),
  value: z.number().nullable(),
  opened_at: z.string(),
  acknowledged_at: z.string().nullable(),
  acknowledged_by: z.string().nullable(),
  resolved_at: z.string().nullable(),
  resolved_by: z.string().nullable(),
  reopened: z.number().int(),
  told_to: z.array(z.string()),
  traces: z.array(z.string()),
  changes: z.array(ChangeSchema).nullable(),
  /** Whether it had more than are named. */
  changes_more: z.boolean(),
});
export type Incident = z.infer<typeof IncidentSchema>;

// A maintenance window: one alert's or monitor's, or every one's where both
// are none; one ended early names who ended it.
const MuteSchema = z.object({
  id: z.string(),
  rule_id: z.string().nullable(),
  monitor_id: z.string().nullable(),
  starts_at: z.string(),
  ends_at: z.string(),
  created_by: z.string(),
  created_at: z.string(),
  ended_by: z.string().nullable(),
});
export type Mute = z.infer<typeof MuteSchema>;

// The three lanes the page reads, each its rows under one key; the alerts'
// beside the starters the plan had no room for, and its sentence on them.
export const AlertListSchema = z.object({
  rules: z.array(AlertSchema),
  starters_withheld: z.array(z.enum(RuleMeasure)),
  starters_detail: z.string().nullable(),
});
export const IncidentListSchema = z.object({ incidents: z.array(IncidentSchema) });
export const MuteListSchema = z.object({ mutes: z.array(MuteSchema) });

export type AlertsAccess =
  | {
      kind: "ok";
      alerts: Alert[];
      /** The starters the project's first trace found no room for, by measure. */
      startersWithheld: RuleMeasure[];
      /** Where any were, the plan's own sentence on what holds more. */
      startersDetail: string | null;
      incidents: Incident[];
      mutes: Mute[];
      /** When it was read, in epoch milliseconds: the page's now. */
      at: number;
    }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type IncidentAccess =
  | { kind: "ok"; incident: Incident }
  /** No incident with that id in this project. */
  | { kind: "missing" }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export { INCIDENTS_MAX } from "@/lib/alerts";

// The 50th, 90th and 99th percentiles of one measure, none where there was nothing.
const PercentilesSchema = z.object({
  p50: z.number().nullable(),
  p90: z.number().nullable(),
  p99: z.number().nullable(),
});
export type Percentiles = z.infer<typeof PercentilesSchema>;

// What one agent's spans of one kind usually did over the whole days read,
// or every agent's together where `agent` is none: durations as the spans'
// own percentiles, tokens, spend and the share failed an hour's.
const AgentHistorySchema = z.object({
  agent: z.string().nullable(),
  kind: z.enum(SpanKind),
  spans: z.number().int(),
  hours: z.number().int(),
  duration_ms: PercentilesSchema,
  tokens_hourly: PercentilesSchema,
  tokens_per_span: PercentilesSchema,
  spend_hourly: PercentilesSchema,
  failure_share: PercentilesSchema,
});
export type AgentHistory = z.infer<typeof AgentHistorySchema>;

const HistorySchema = z.object({
  from: z.string(),
  to: z.string(),
  agents: z.array(AgentHistorySchema),
});
export type History = z.infer<typeof HistorySchema>;

/** Each agent's history, for an alert's threshold to start from; none where
 *  it cannot be read, which the form goes on without. */
export const fetchHistory = cache(async (projectId: string): Promise<History | null> => {
  const read = await readLane("fetchHistory", projectId, "/history", HistorySchema);
  return read.kind === "ok" ? read.value : null;
});

// Where one objective stands: its burns, and its budget over the days read.
const StandingSchema = z.object({
  rule_id: z.string(),
  hour_burn: z.number(),
  six_hour_burn: z.number(),
  days: z.number().int(),
  traces: z.number().int(),
  missed: z.number().int(),
  budget_left: z.number(),
});
export type Standing = z.infer<typeof StandingSchema>;

/** Where each of the project's objectives stands, by its alert's id; none
 *  where it cannot be read. */
export const fetchObjectives = cache(async (projectId: string): Promise<Standing[] | null> => {
  const read = await readLane(
    "fetchObjectives",
    projectId,
    "/objectives",
    z.object({ objectives: z.array(StandingSchema) }),
  );
  return read.kind === "ok" ? read.value.objectives : null;
});

/** What the alerts page reads: the alerts by name, the incidents — those not
 *  resolved first, then the newest — and the maintenance windows; read
 *  together, so the page is one moment's. */
export const fetchAlerts = cache(async (projectId: string): Promise<AlertsAccess> => {
  const at = Date.now();
  const [alerts, incidents, mutes] = await Promise.all([
    readLane("fetchAlerts", projectId, "/rules", AlertListSchema),
    readLane("fetchAlerts", projectId, "/incidents", IncidentListSchema),
    readLane("fetchAlerts", projectId, "/mutes", MuteListSchema),
  ]);
  const reads = [alerts, incidents, mutes];
  if (reads.some((read) => read.kind === "forbidden")) return { kind: "forbidden" };
  if (alerts.kind !== "ok" || incidents.kind !== "ok" || mutes.kind !== "ok") {
    return { kind: "unavailable" };
  }
  return {
    kind: "ok",
    alerts: alerts.value.rules,
    startersWithheld: alerts.value.starters_withheld,
    startersDetail: alerts.value.starters_detail,
    incidents: incidents.value.incidents,
    mutes: mutes.value.mutes,
    at,
  };
});

/** The incidents whose notices showed the trace `traceId`, the newest first,
 *  as a trace's page links them; none where they cannot be read, which the
 *  page goes on without, and none asked for a trace id the path cannot be. */
export const fetchTraceIncidents = cache(
  async (projectId: string, traceId: string): Promise<Incident[]> => {
    if (!TraceIdSchema.safeParse(traceId).success) return [];
    const read = await readLane(
      "fetchTraceIncidents",
      projectId,
      `/incidents?trace=${encodeURIComponent(traceId)}`,
      IncidentListSchema,
    );
    return read.kind === "ok" ? read.value.incidents : [];
  },
);

/** One incident, by its id. An id the path cannot be — not a UUID — is read
 *  as missing, as the server would answer one that is not the project's. */
export const fetchIncident = cache(
  async (projectId: string, incidentId: string): Promise<IncidentAccess> => {
    const read = await readLane(
      "fetchIncident",
      projectId,
      `/incidents/${encodeURIComponent(incidentId)}`,
      IncidentSchema,
      MISSING,
    );
    return read.kind === "ok" ? { kind: "ok", incident: read.value } : read;
  },
);
