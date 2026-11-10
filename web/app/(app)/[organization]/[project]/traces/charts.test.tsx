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
  ReferenceLine: ({ x }: { x: string }) => <span data-testid="mark" data-x={x} />,
  CartesianGrid: () => null,
  XAxis: () => null,
  YAxis: () => null,
}));

// The tooltip as recharts draws it over the first bar: its label alone.
vi.mock("@/components/ui/chart", () => ({
  ChartContainer: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  ChartLegend: () => null,
  ChartLegendContent: () => null,
  ChartTooltip: ({ content }: { content: ReactNode }) => content,
  ChartTooltipContent: ({
    labelFormatter,
  }: {
    labelFormatter?: (label: unknown, payload: { payload: { at: string } }[]) => ReactNode;
  }) => <div data-testid="tooltip">{labelFormatter?.(undefined, [{ payload: { at: BAR_AT } }])}</div>,
}));

import { TraceCharts, TraceHistogramStrip } from "@/components/traces/charts";

const BAR_AT = "2026-10-30T09:00:00Z";

const HISTOGRAM = {
  days: 1,
  bucket_seconds: 3600,
  utc_offset: 0,
  score: null as string | null,
  bars: [
    {
      at: BAR_AT,
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
    render(<TraceCharts histogram={HISTOGRAM} marks={[]} />);
    expect(linesOf("Time to first token")).toEqual([expect.objectContaining({ p50: 320, p95: 900 })]);
    expect(linesOf("Duration")).toEqual([expect.objectContaining({ p50: 1500, p95: 4000 })]);
    const series = within(screen.getByRole("region", { name: "Time to first token" }))
      .getAllByTestId("series")
      .map((line) => line.textContent);
    expect(series).toEqual(["p50", "p95"]);
  });

  it("draws a score's mean where the list is narrowed to one, and no score chart where not", () => {
    const { unmount } = render(<TraceCharts histogram={HISTOGRAM} marks={[]} />);
    expect(screen.queryByRole("region", { name: "Score" })).toBeNull();
    unmount();
    const scored = { ...HISTOGRAM, score: "relevance", bars: [{ ...HISTOGRAM.bars[0]!, score_mean: 0.82 }] };
    render(<TraceCharts histogram={scored} marks={[]} />);
    expect(linesOf("Score")).toEqual([expect.objectContaining({ score: 0.82 })]);
    expect(screen.getByRole("heading", { name: /^The mean relevance score/ })).toBeTruthy();
  });

  it("draws the cost beside the tokens, each bar what its traces spent", () => {
    render(<TraceCharts histogram={HISTOGRAM} marks={[]} />);
    const cost = within(screen.getByRole("region", { name: "Cost" }));
    expect(JSON.parse(cost.getByTestId("bars").dataset.rows ?? "null")).toEqual([
      expect.objectContaining({ cost: 0.25 }),
    ]);
    expect(cost.getAllByTestId("series").map((bar) => bar.textContent)).toEqual(["cost"]);
  });

  it("marks each change's bar on every chart, its bars and its lines, and names the bar's changes in its tooltip", () => {
    const changes = ["support: release 1.4.2 in production", "triage: model gpt-4o-mini"];
    render(<TraceCharts histogram={HISTOGRAM} marks={[{ at: BAR_AT, changes }]} />);
    for (const name of ["Traces", "Tokens", "Cost", "Duration", "Time to first token"]) {
      const chart = within(screen.getByRole("region", { name }));
      expect(chart.getAllByTestId("mark").map((mark) => mark.dataset.x), name).toEqual([BAR_AT]);
      for (const line of changes) expect(chart.getByTestId("tooltip"), name).toHaveTextContent(line);
    }
  });

  it("draws no mark where no change was read", () => {
    render(<TraceCharts histogram={HISTOGRAM} marks={[]} />);
    expect(screen.queryAllByTestId("mark")).toEqual([]);
  });
});

describe("TraceHistogramStrip", () => {
  it("marks each change's bar on the strip above the table too", () => {
    render(
      <TraceHistogramStrip
        histogram={HISTOGRAM}
        marks={[{ at: BAR_AT, changes: ["support: release 1.4.2 in production"] }]}
      />,
    );
    const strip = within(screen.getByRole("region", { name: "Over the window" }));
    expect(strip.getAllByTestId("mark").map((mark) => mark.dataset.x)).toEqual([BAR_AT]);
    expect(strip.getByTestId("tooltip")).toHaveTextContent("support: release 1.4.2 in production");
  });
});
