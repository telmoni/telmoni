import { cache } from "react";
import { z } from "zod";

import { MetricsGroup } from "@/lib/types/enums";

import { readLane } from "./traces";

// One hour's or day's totals, as telemetry's rollups lane answers them.
const PointSchema = z.object({
  at: z.string(),
  spans: z.number(),
  failed: z.number(),
  traces: z.number(),
  failed_traces: z.number(),
  input_tokens: z.number(),
  output_tokens: z.number(),
  cache_read_input_tokens: z.number(),
  cache_write_input_tokens: z.number(),
  spend: z.number(),
  trace_duration_ms: z.object({
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
 *  and by the day for longer. */
export const fetchRollups = cache(async (projectId: string, days: number): Promise<RollupsAccess> => {
  const query = new URLSearchParams({ days: String(days), step: days <= 1 ? "hour" : "day" });
  const read = await readLane("fetchRollups", projectId, `/rollups?${query}`, RollupsSchema);
  return read.kind === "ok" ? { kind: "ok", rollups: read.value } : read;
});
