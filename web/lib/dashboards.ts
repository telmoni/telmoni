import type { RollupPoint } from "@/lib/server/entities/dashboards";

// What the dashboards page draws of the project's hourly totals: a bar an
// hour or a day for its charts, and each group's totals for its table.

/** The columns the totals may be grouped by, as the lane takes them. */
export const ROLLUP_GROUPS = ["model", "agent", "provider", "environment", "release"] as const;
export type RollupGroup = (typeof ROLLUP_GROUPS)[number];

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
    ok: point.runs - point.failed_runs,
    failed: point.failed_runs,
    p50: point.run_duration_ms.p50,
    p95: point.run_duration_ms.p95,
    p99: point.run_duration_ms.p99,
  }));
}

/** One group's totals over the window. */
export type GroupTotal = {
  value: string;
  spend: number;
  tokens: number;
  runs: number;
  failedRuns: number;
};

/** Each group's totals over the window, the costliest first, the most
 *  tokens breaking a tie; a span carrying no value of it as its own row. */
export function groupTotals(points: readonly RollupPoint[]): GroupTotal[] {
  const totals = new Map<string, GroupTotal>();
  for (const point of points) {
    const value = point.value ?? "";
    const held = totals.get(value) ?? { value, spend: 0, tokens: 0, runs: 0, failedRuns: 0 };
    held.spend += point.spend;
    held.tokens += point.input_tokens + point.output_tokens;
    held.runs += point.runs;
    held.failedRuns += point.failed_runs;
    totals.set(value, held);
  }
  return [...totals.values()].sort((a, b) => b.spend - a.spend || b.tokens - a.tokens);
}
