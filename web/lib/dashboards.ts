import type { RollupPoint } from "@/lib/server/entities/dashboards";
import type { TokenBar, TokenCounts } from "@/lib/traces";
import { z } from "zod";
import { ChartMeasure } from "@/lib/types/enums";

// What the Spend page draws of the project's hourly totals: a bar an hour or
// a day for each chart.

/** The most charts one dashboard draws, as the server takes them. */
export const DASHBOARD_CHARTS_MAX = 12;

export const SPEND_CHART_LABELS: Record<ChartMeasure, string> = {
  spend: "Spend",
  tokens: "Tokens",
  traces: "Traces",
  latency: "Latency",
  first_token: "Time to first token",
  output_speed: "Output speed",
};

const LayoutSchema = z.array(z.object({
  measure: z.enum(ChartMeasure),
  width: z.enum(["half", "full"]),
  visible: z.boolean(),
})).length(Object.values(ChartMeasure).length).refine(
  (rows) => new Set(rows.map((row) => row.measure)).size === rows.length,
);
export type SpendLayout = z.infer<typeof LayoutSchema>;
export const DEFAULT_SPEND_LAYOUT: SpendLayout = Object.values(ChartMeasure).map((measure) => ({
  measure, width: "half", visible: true,
}));

/** Invalid or incomplete stored layouts use the current complete chart set. */
export function readSpendLayout(stored: string | null): SpendLayout {
  if (!stored) return DEFAULT_SPEND_LAYOUT;
  try {
    const result = LayoutSchema.safeParse(JSON.parse(stored));
    return result.success ? result.data : DEFAULT_SPEND_LAYOUT;
  } catch {
    return DEFAULT_SPEND_LAYOUT;
  }
}

/** The most values a split read from the spans names before the rest are
 *  summed as one, as the server reads them. */
export const ROLLUP_GROUP_TOP = 10;

/** One bar of the charts. */
export type Bar = {
  at: string;
  spend: number;
  ok: number;
  failed: number;
  p50: number | null;
  p95: number | null;
  p99: number | null;
};

/** Each hour's or day's totals as a bar, oldest first. */
export function barsOf(points: readonly RollupPoint[]): Bar[] {
  return points.map((point) => ({
    at: point.at,
    spend: point.spend,
    ok: point.traces - point.failed_traces,
    failed: point.failed_traces,
    p50: point.trace_duration_ms.p50,
    p95: point.trace_duration_ms.p95,
    p99: point.trace_duration_ms.p99,
  }));
}

/** Each hour's or day's tokens as a bar, oldest first, by model where the
 *  points are grouped by it and under one name where they are not. A value
 *  with no token is left out: the totals group every span, and only a model
 *  call's carries a model. */
export function tokenBarsOf(points: readonly RollupPoint[]): TokenBar[] {
  const bars = new Map<string, Map<string, TokenCounts>>();
  for (const point of points) {
    const models = bars.get(point.at) ?? new Map<string, TokenCounts>();
    bars.set(point.at, models);
    if (point.input_tokens + point.output_tokens === 0) continue;
    models.set(point.value ?? "", {
      input_tokens: point.input_tokens,
      output_tokens: point.output_tokens,
      cache_read_input_tokens: point.cache_read_input_tokens,
      cache_write_input_tokens: point.cache_write_input_tokens,
      reasoning_output_tokens: point.reasoning_output_tokens,
    });
  }
  return [...bars]
    .map(([at, models]) => ({ at, tokens_by_model: Object.fromEntries(models) }))
    .sort((a, b) => a.at.localeCompare(b.at));
}
