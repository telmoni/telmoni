// @vitest-environment jsdom
import { render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";

// Recharts measures the box it draws in, which jsdom has none of: each chart
// is drawn here as the rows it was handed and the series it draws of them.
vi.mock("recharts", () => ({
  LineChart: ({ data, children }: { data: unknown; children: ReactNode }) => (
    <div data-testid="lines" data-rows={JSON.stringify(data)}>
      {children}
    </div>
  ),
  BarChart: ({ data, children }: { data: unknown; children: ReactNode }) => (
    <div data-testid="bars" data-rows={JSON.stringify(data)}>
      {children}
    </div>
  ),
  Line: ({ dataKey }: { dataKey: string }) => <span data-testid="series">{dataKey}</span>,
  Bar: ({ dataKey }: { dataKey: string }) => <span data-testid="series">{dataKey}</span>,
  CartesianGrid: () => null,
  XAxis: () => null,
  YAxis: () => null,
}));

vi.mock("@/components/ui/chart", () => ({
  ChartContainer: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  ChartLegend: () => null,
  ChartLegendContent: () => null,
  ChartTooltip: () => null,
  ChartTooltipContent: () => null,
}));

import { TraceCharts } from "@/components/traces/charts";

const HISTOGRAM = {
  days: 1,
  bucket_seconds: 3600,
  utc_offset: 0,
  score: null as string | null,
  bars: [
    {
      at: "2026-10-30T09:00:00Z",
      ok: 3,
      error: 1,
      cancelled: 0,
      tokens: 1200,
      spend: 0.25,
      duration_p50_ms: 1500,
      duration_p95_ms: 4000,
      time_to_first_token_p50_ms: 320,
      time_to_first_token_p95_ms: 900,
      score_mean: null as number | null,
    },
  ],
};

/** The rows the line chart under `name` was handed. */
function linesOf(name: string): unknown {
  const chart = within(screen.getByRole("region", { name })).getByTestId("lines");
  return JSON.parse(chart.dataset.rows ?? "null");
}

describe("TraceCharts", () => {
  it("draws time to first token from its own median and 95th percentile, never the duration's", () => {
    render(<TraceCharts histogram={HISTOGRAM} />);
    expect(linesOf("Time to first token")).toEqual([expect.objectContaining({ p50: 320, p95: 900 })]);
    expect(linesOf("Duration")).toEqual([expect.objectContaining({ p50: 1500, p95: 4000 })]);
    const series = within(screen.getByRole("region", { name: "Time to first token" }))
      .getAllByTestId("series")
      .map((line) => line.textContent);
    expect(series).toEqual(["p50", "p95"]);
  });

  it("draws a score's mean where the list is narrowed to one, and no score chart where not", () => {
    const { unmount } = render(<TraceCharts histogram={HISTOGRAM} />);
    expect(screen.queryByRole("region", { name: "Score" })).toBeNull();
    unmount();
    const scored = { ...HISTOGRAM, score: "relevance", bars: [{ ...HISTOGRAM.bars[0]!, score_mean: 0.82 }] };
    render(<TraceCharts histogram={scored} />);
    expect(linesOf("Score")).toEqual([expect.objectContaining({ score: 0.82 })]);
    expect(screen.getByRole("heading", { name: /^The mean relevance score/ })).toBeTruthy();
  });

  it("draws the cost beside the tokens, each bar what its runs spent", () => {
    render(<TraceCharts histogram={HISTOGRAM} />);
    const cost = within(screen.getByRole("region", { name: "Cost" }));
    expect(JSON.parse(cost.getByTestId("bars").dataset.rows ?? "null")).toEqual([
      expect.objectContaining({ cost: 0.25 }),
    ]);
    expect(cost.getAllByTestId("series").map((bar) => bar.textContent)).toEqual(["cost"]);
  });
});
