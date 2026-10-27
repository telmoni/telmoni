import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import {
  IncidentStatus,
  MissingData,
  PausedBy,
  RuleAggregation,
  RuleLevel,
  RuleMeasure,
  RuleOperator,
  RuleWindow,
  Severity,
  SpanKind,
  SpanStatus,
} from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";
import { readLane } from "./traces";

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
  status: z.array(z.enum(SpanStatus)).optional(),
  environment: z.array(z.string()).optional(),
  release: z.array(z.string()).optional(),
});
export type RuleFilters = z.infer<typeof RuleFiltersSchema>;

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
  /** A member, by hand, or its plan, past the organization's number. */
  paused: z.enum(PausedBy).nullable(),
  paused_at: z.string().nullable(),
  /** None before its first evaluation, or where its window held no span. */
  value: z.number().nullable(),
  evaluated_at: z.string().nullable(),
  severity: z.enum(Severity),
  severity_since: z.string(),
  connectors: z.array(AlertLinkSchema),
  created_at: z.string(),
  updated_at: z.string(),
});
export type Alert = z.infer<typeof AlertSchema>;

// One incident: its alert, where it stands, who took or resolved it, where
// its notices went — the connections, the feed apart — and the runs they
// showed, oldest first.
export const IncidentSchema = z.object({
  id: z.string(),
  rule_id: z.string(),
  rule_name: z.string(),
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
  runs: z.array(z.string()),
});
export type Incident = z.infer<typeof IncidentSchema>;

// A maintenance window: one alert's, or every alert's where `rule_id` is
// none; one ended early names who ended it.
const MuteSchema = z.object({
  id: z.string(),
  rule_id: z.string().nullable(),
  starts_at: z.string(),
  ends_at: z.string(),
  created_by: z.string(),
  created_at: z.string(),
  ended_by: z.string().nullable(),
});
export type Mute = z.infer<typeof MuteSchema>;

export type AlertsAccess =
  | {
      kind: "ok";
      alerts: Alert[];
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

/** What the alerts page reads: the alerts by name, the incidents — those not
 *  resolved first, then the newest — and the maintenance windows; read
 *  together, so the page is one moment's. */
export const fetchAlerts = cache(async (projectId: string): Promise<AlertsAccess> => {
  const at = Date.now();
  const [alerts, incidents, mutes] = await Promise.all([
    readLane("fetchAlerts", projectId, "/rules", z.object({ rules: z.array(AlertSchema) })),
    readLane("fetchAlerts", projectId, "/incidents", z.object({ incidents: z.array(IncidentSchema) })),
    readLane("fetchAlerts", projectId, "/mutes", z.object({ mutes: z.array(MuteSchema) })),
  ]);
  const reads = [alerts, incidents, mutes];
  if (reads.some((read) => read.kind === "forbidden")) return { kind: "forbidden" };
  if (alerts.kind !== "ok" || incidents.kind !== "ok" || mutes.kind !== "ok") {
    return { kind: "unavailable" };
  }
  return {
    kind: "ok",
    alerts: alerts.value.rules,
    incidents: incidents.value.incidents,
    mutes: mutes.value.mutes,
    at,
  };
});

/** The incidents whose notices showed the run `runId`, the newest first, as
 *  a trace's page links them; none where they cannot be read, which the page
 *  goes on without, and none asked for a run's id the path cannot be. */
export const fetchRunIncidents = cache(
  async (projectId: string, runId: string): Promise<Incident[]> => {
    if (!z.guid().safeParse(runId).success) return [];
    const read = await readLane(
      "fetchRunIncidents",
      projectId,
      `/incidents?run=${encodeURIComponent(runId)}`,
      z.object({ incidents: z.array(IncidentSchema) }),
    );
    return read.kind === "ok" ? read.value.incidents : [];
  },
);

/** One incident, by its id. An id the path cannot be — not a UUID — is read
 *  as missing, as the server would answer one that is not the project's. */
export const fetchIncident = cache(
  async (projectId: string, incidentId: string): Promise<IncidentAccess> => {
    if (!projectId) return { kind: "unavailable" };
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/v1/telemetry/incidents/${encodeURIComponent(incidentId)}`,
        { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      );
      if (res.status === 403) return { kind: "forbidden" };
      if (res.status === 404 || res.status === 400) return { kind: "missing" };
      if (!res.ok) {
        logger.warn({ fetcher: "fetchIncident", status: res.status }, "entities: upstream refused");
        return { kind: "unavailable" };
      }
      const parsed = IncidentSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchIncident", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return { kind: "unavailable" };
      }
      return { kind: "ok", incident: parsed.data };
    } catch (err) {
      logger.warn(
        {
          fetcher: "fetchIncident",
          error: err instanceof Error ? err.message : String(err),
        },
        "entities: fetch failed",
      );
      return { kind: "unavailable" };
    }
  },
);
