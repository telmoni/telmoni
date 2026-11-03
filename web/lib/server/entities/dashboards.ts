import { cache } from "react";
import { z } from "zod";

import { MetricsGroup, ViewKind } from "@/lib/types/enums";

import { DashboardViewListSchema, readLane, type DashboardView } from "./traces";

// One hour's or day's totals, of one group's value where grouped, as
// telemetry's rollups lane answers them.
const PointSchema = z.object({
  at: z.string(),
  value: z.string().optional(),
  /** The values past the top that spent most, summed: a split from the spans. */
  others: z.boolean().optional(),
  spans: z.number(),
  failed: z.number(),
  runs: z.number(),
  failed_runs: z.number(),
  input_tokens: z.number(),
  output_tokens: z.number(),
  cache_read_input_tokens: z.number(),
  cache_write_input_tokens: z.number(),
  spend: z.number(),
  run_duration_ms: z.object({
    p50: z.number().nullable(),
    p95: z.number().nullable(),
    p99: z.number().nullable(),
  }),
});
export type RollupPoint = z.infer<typeof PointSchema>;

export const RollupsSchema = z.object({
  days: z.number(),
  step: z.enum(["hour", "day"]),
  group: z.enum(MetricsGroup).nullable(),
  tag: z.string().optional(),
  points: z.array(PointSchema),
  truncated: z.boolean(),
});
export type Rollups = z.infer<typeof RollupsSchema>;

export type RollupsAccess =
  | { kind: "ok"; rollups: Rollups }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** The project's hourly totals over the last `days`, by the hour for a day
 *  and by the day for longer, split where `split` names a spend chart's
 *  group; a split by a tag, a user or a session no further back than the
 *  plan keeps the spans, its answer's `days` saying how far. */
export const fetchRollups = cache(
  async (projectId: string, days: number, split?: MetricsGroup, tag?: string): Promise<RollupsAccess> => {
    const query = new URLSearchParams({ days: String(days), step: days <= 1 ? "hour" : "day" });
    if (split) query.set("group", split);
    if (tag) query.set("tag", tag);
    const read = await readLane("fetchRollups", projectId, `/rollups?${query}`, RollupsSchema);
    return read.kind === "ok" ? { kind: "ok", rollups: read.value } : read;
  },
);


export type DashboardsAccess =
  | { kind: "ok"; dashboards: DashboardView[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** The project's saved dashboards, by name. */
export const fetchDashboards = cache(async (projectId: string): Promise<DashboardsAccess> => {
  const read = await readLane(
    "fetchDashboards",
    projectId,
    `/views?kind=${ViewKind.Dashboard}`,
    DashboardViewListSchema,
  );
  return read.kind === "ok" ? { kind: "ok", dashboards: read.value.views } : read;
});
