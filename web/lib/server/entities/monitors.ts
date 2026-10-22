import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { IncidentDetail, IncidentResolution, MonitorState, SpanStatus } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";

// A monitor's last run that ended, as its row keeps it: none until one has.
const LastRunSchema = z.object({
  run_id: z.string(),
  started_at: z.string(),
  ended_at: z.string(),
  status: z.enum(SpanStatus),
  steps: z.number().int().nullable(),
  cost_usd: z.number().nullable(),
});

const BoundsSchema = z.object({
  stall_after_s: z.number().int().nullable(),
  max_steps: z.number().int().nullable(),
  max_cost_usd: z.number().nullable(),
});
export type Bounds = z.infer<typeof BoundsSchema>;

// A monitor as the lane answers it: an agent's contract, its state, when its
// next run is due, its last run and its last success.
export const MonitorSchema = z.object({
  id: z.string(),
  slug: z.string(),
  name: z.string(),
  agent: z.string(),
  schedule: z.string().nullable(),
  interval_s: z.number().int().nullable(),
  timezone: z.string(),
  grace_s: z.number().int().nullable(),
  expected_duration_s: z.number().int().nullable(),
  bounds: BoundsSchema,
  state: z.enum(MonitorState),
  state_changed_at: z.string(),
  next_due_at: z.string().nullable(),
  last_run: LastRunSchema.nullable(),
  last_success_at: z.string().nullable(),
  created_at: z.string(),
  updated_at: z.string(),
});
export type Monitor = z.infer<typeof MonitorSchema>;

// A run of the monitor's still going, held to the tighter of the monitor's
// bounds and its code's.
const OpenRunSchema = z.object({
  run_id: z.string(),
  source: z.enum(["ping", "ingest"]),
  started_at: z.string(),
  last_heard_at: z.string(),
  ends_by: z.string(),
  steps: z.number().int(),
  input_tokens: z.number().int(),
  output_tokens: z.number().int(),
  cost_usd: z.number(),
  bounds: BoundsSchema,
});
export type OpenRun = z.infer<typeof OpenRunSchema>;

const IncidentSchema = z.object({
  id: z.string(),
  cause: z.enum(MonitorState),
  detail: z.enum(IncidentDetail),
  opened_at: z.string(),
  resolved_at: z.string().nullable(),
  resolution: z.enum(IncidentResolution).nullable(),
  flapping: z.boolean(),
});

// One monitor as its page reads it: its runs still going, and its incidents,
// the open one first and then the last resolved.
const MonitorDetailSchema = MonitorSchema.extend({
  open_runs: z.array(OpenRunSchema),
  incidents: z.array(IncidentSchema),
});
export type MonitorDetail = z.infer<typeof MonitorDetailSchema>;

const MonitorListSchema = z.object({ monitors: z.array(MonitorSchema) });
const TimezonesSchema = z.object({ zones: z.array(z.string()) });

// A ping key as the list shows it: never the key, its prefix and first
// characters alone.
const PingKeySchema = z.object({
  id: z.string(),
  prefix: z.string(),
  created_at: z.string(),
  last_ping_at: z.string().nullable(),
  retired_at: z.string().nullable(),
});
export type PingKey = z.infer<typeof PingKeySchema>;
const PingKeyListSchema = z.object({ keys: z.array(PingKeySchema) });

export type MonitorsAccess =
  | { kind: "ok"; monitors: Monitor[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type MonitorAccess =
  | { kind: "ok"; monitor: MonitorDetail }
  /** No monitor by that slug in this project. */
  | { kind: "missing" }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export type PingKeysAccess =
  | { kind: "ok"; keys: PingKey[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

type Read<T> =
  | { kind: "ok"; value: T }
  | { kind: "missing" }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** One of telemetry's monitor lanes as the project reads it, its answer held
 *  to `schema`: missing on a 404, forbidden on a 403, unavailable on anything
 *  else it cannot read. */
async function readLane<T>(
  fetcher: string,
  projectId: string,
  path: string,
  schema: z.ZodType<T>,
): Promise<Read<T>> {
  if (!projectId) return { kind: "unavailable" };
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  try {
    const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry${path}`, {
      headers: projectHeaders(ctx, projectId),
      cache: "no-store",
    });
    if (res.status === 404) return { kind: "missing" };
    if (res.status === 403) return { kind: "forbidden" };
    if (!res.ok) {
      logger.warn({ fetcher, status: res.status }, "entities: upstream refused");
      return { kind: "unavailable" };
    }
    const parsed = schema.safeParse(await res.json());
    if (!parsed.success) {
      logger.warn({ fetcher, issues: parsed.error.issues }, "entities: upstream shape mismatch");
      return { kind: "unavailable" };
    }
    return { kind: "ok", value: parsed.data };
  } catch (err) {
    logger.warn(
      { fetcher, error: err instanceof Error ? err.message : String(err) },
      "entities: fetch failed",
    );
    return { kind: "unavailable" };
  }
}

/** The project's monitors, the worst state first as the lane sorts them,
 *  from Postgres alone: forbidden on a 403, unavailable on anything else it
 *  cannot read. */
export const fetchMonitors = cache(async (projectId: string): Promise<MonitorsAccess> => {
  const read = await readLane("fetchMonitors", projectId, "/monitors", MonitorListSchema);
  if (read.kind === "ok") return { kind: "ok", monitors: read.value.monitors };
  return read.kind === "missing" ? { kind: "unavailable" } : read;
});

/** One of the project's monitors by its slug, with its runs still going and
 *  its incidents. A slug of no monitor's shape is read as missing, as the
 *  server would answer it. */
export const fetchMonitor = cache(
  async (projectId: string, slug: string): Promise<MonitorAccess> => {
    if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(slug)) return { kind: "missing" };
    const read = await readLane(
      "fetchMonitor",
      projectId,
      `/monitors/${encodeURIComponent(slug)}`,
      MonitorDetailSchema,
    );
    return read.kind === "ok" ? { kind: "ok", monitor: read.value } : read;
  },
);

/** Every zone a monitor may be in, as the server's zone database names them;
 *  `null` when the list cannot be read. */
export const fetchTimezones = cache(async (projectId: string): Promise<string[] | null> => {
  const read = await readLane("fetchTimezones", projectId, "/timezones", TimezonesSchema);
  return read.kind === "ok" ? read.value.zones : null;
});

/** The project's ping keys, the live ones first, each newest first. */
export const fetchPingKeys = cache(async (projectId: string): Promise<PingKeysAccess> => {
  const read = await readLane("fetchPingKeys", projectId, "/ping-keys", PingKeyListSchema);
  if (read.kind === "ok") return { kind: "ok", keys: read.value.keys };
  return read.kind === "missing" ? { kind: "unavailable" } : read;
});
