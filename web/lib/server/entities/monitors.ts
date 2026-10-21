import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { MonitorState, SpanStatus } from "@/lib/types/enums";

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

// A monitor as the lane answers it: an agent's contract, its state, when its
// next run is due, and its last run.
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
  bounds: z.object({
    stall_after_s: z.number().int().nullable(),
    max_steps: z.number().int().nullable(),
    max_cost_usd: z.number().nullable(),
  }),
  state: z.enum(MonitorState),
  state_changed_at: z.string(),
  next_due_at: z.string().nullable(),
  last_run: LastRunSchema.nullable(),
  created_at: z.string(),
  updated_at: z.string(),
});
export type Monitor = z.infer<typeof MonitorSchema>;

const MonitorListSchema = z.object({ monitors: z.array(MonitorSchema) });

export type MonitorsAccess =
  | { kind: "ok"; monitors: Monitor[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** The project's monitors, the worst state first as the lane sorts them,
 *  from Postgres alone: forbidden on a 403, unavailable on anything else it
 *  cannot read. */
export const fetchMonitors = cache(async (projectId: string): Promise<MonitorsAccess> => {
  if (!projectId) return { kind: "unavailable" };
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  try {
    const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/monitors`, {
      headers: projectHeaders(ctx, projectId),
      cache: "no-store",
    });
    if (res.status === 403) return { kind: "forbidden" };
    if (!res.ok) {
      logger.warn({ fetcher: "fetchMonitors", status: res.status }, "entities: upstream refused");
      return { kind: "unavailable" };
    }
    const parsed = MonitorListSchema.safeParse(await res.json());
    if (!parsed.success) {
      logger.warn(
        { fetcher: "fetchMonitors", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return { kind: "unavailable" };
    }
    return { kind: "ok", monitors: parsed.data.monitors };
  } catch (err) {
    logger.warn(
      { fetcher: "fetchMonitors", error: err instanceof Error ? err.message : String(err) },
      "entities: fetch failed",
    );
    return { kind: "unavailable" };
  }
});
