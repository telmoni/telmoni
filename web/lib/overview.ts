// What a project's overview cards show, worked out from what its pages read.

import type { RollupPoint } from "@/lib/server/entities/dashboards";

export type DayTotals = { runs: number; failedRuns: number; spend: number; tokens: number };

/** The window's hourly totals summed. */
export function totalsOf(points: readonly RollupPoint[]): DayTotals {
  return points.reduce<DayTotals>(
    (sum, point) => ({
      runs: sum.runs + point.runs,
      failedRuns: sum.failedRuns + point.failed_runs,
      spend: sum.spend + point.spend,
      tokens: sum.tokens + point.input_tokens + point.output_tokens,
    }),
    { runs: 0, failedRuns: 0, spend: 0, tokens: 0 },
  );
}
