import type { RollupPoint } from "@/lib/server/entities/dashboards";

// What the Spend page draws of the project's hourly totals: a bar an hour or
// a day for each chart.

/** The most charts one dashboard draws, as the server takes them. */
export const DASHBOARD_CHARTS_MAX = 12;

/** The most values a split read from the spans names before the rest are
 *  summed as one, as the server reads them. */
export const ROLLUP_GROUP_TOP = 10;

/** One bar of the charts. */
export type Bar = {
  at: string;
  spend: number;
  input: number;
  output: number;
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
    input: point.input_tokens,
    output: point.output_tokens,
    ok: point.traces - point.failed_traces,
    failed: point.failed_traces,
    p50: point.trace_duration_ms.p50,
    p95: point.trace_duration_ms.p95,
    p99: point.trace_duration_ms.p99,
  }));
}
