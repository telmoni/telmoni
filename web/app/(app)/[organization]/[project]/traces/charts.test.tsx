// @vitest-environment jsdom
import { fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeAll, describe, expect, it, vi } from "vitest";

// The picker's menu scrolls its choice into view, which jsdom has no way to do.
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

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
  Line: ({ dataKey, connectNulls, dot }: { dataKey: string; connectNulls?: boolean; dot?: unknown }) => (
    <span data-testid="series" data-connected={connectNulls} data-dot={JSON.stringify(dot)}>{dataKey}</span>
  ),
  Bar: ({ dataKey }: { dataKey: string }) => <span data-testid="series">{dataKey}</span>,
  ReferenceLine: ({ x }: { x: string }) => <span data-testid="mark" data-x={x} />,
  CartesianGrid: () => null,
  XAxis: ({ tickFormatter }: { tickFormatter?: (iso: string) => string }) => (
    <span data-testid="axis-time">{tickFormatter?.("2026-11-01T00:00:00Z")}</span>
  ),
  YAxis: ({ tickFormatter }: { tickFormatter?: (value: number) => string }) => (
    <span data-testid="axis-value">{tickFormatter?.(50)}</span>
  ),
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

import { OutputSpeedChart, TokenChart, TraceCharts, TraceHistogramStrip } from "@/components/traces/charts";
import type { OutputSpeedBar } from "@/lib/traces";

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
      faults: { rate_limited: 1, timeout: 1 },
      tokens: 1200,
      tokens_by_model: {
        "model-a": {
          input_tokens: 1000,
          output_tokens: 200,
          cache_read_input_tokens: 200,
          cache_write_input_tokens: 100,
          reasoning_output_tokens: 50,
        },
      },
      spend: 0.25,
      output_speed_by_model: { "model-a": { p50: 50, p05: 5 } },
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
  it("narrows all five token segments and the weighted share with the model picker", async () => {
    const first = HISTOGRAM.bars[0]!;
    const other = {
      input_tokens: 9000,
      output_tokens: 100,
      cache_read_input_tokens: 0,
      cache_write_input_tokens: 0,
      reasoning_output_tokens: 0,
    };
    render(
      <TokenChart
        days={1}
        bars={[{ at: first.at, tokens_by_model: { ...first.tokens_by_model, "model-b": other } }]}
      />,
    );
    expect(screen.getByText("Cache share 2%")).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Token model" }), { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("option", { name: "model-a" }));
    expect(screen.getByText("Cache share 20%")).toBeInTheDocument();
    expect(JSON.parse(screen.getByTestId("bars").dataset.rows ?? "null")).toEqual([
      expect.objectContaining({ input: 700, cacheRead: 200, cacheWrite: 100, output: 150, reasoning: 50 }),
    ]);
    expect(screen.getAllByTestId("series").map((series) => series.textContent)).toEqual([
      "input",
      "cacheRead",
      "cacheWrite",
      "output",
      "reasoning",
    ]);
  });

  it("draws overlapping fault categories independently of trace status", () => {
    render(<TraceCharts histogram={HISTOGRAM} marks={[]} />);
    expect(linesOf("Traces with faults")).toEqual([expect.objectContaining({ rate_limited: 1, timeout: 1, ok: 3, error: 1 })]);
    expect(screen.getByText("Traces containing each fault; categories can overlap.")).toBeInTheDocument();
    // A class the window never met is no line.
    const series = within(screen.getByRole("region", { name: "Traces with faults" }))
      .getAllByTestId("series")
      .map((line) => line.textContent);
    expect(series).toEqual(["rate_limited", "timeout"]);
  });
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
    for (const name of ["Traces", "Tokens", "Cost", "Duration", "Time to first token", "Output speed"]) {
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

describe("OutputSpeedChart", () => {
  it("labels Spend's day and hour bars in UTC", () => {
    const { rerender } = render(<OutputSpeedChart bars={[]} days={7} bucketSeconds={86400} timeZone="UTC" />);
    const at = new Date("2026-11-01T00:00:00Z");
    expect(screen.getByTestId("axis-time")).toHaveTextContent(at.toLocaleDateString([], { month: "short", day: "numeric", timeZone: "UTC" }));
    rerender(<OutputSpeedChart bars={[]} days={1} bucketSeconds={3600} timeZone="UTC" />);
    expect(screen.getByTestId("axis-time")).toHaveTextContent(at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", timeZone: "UTC" }));
  });
  const bars: OutputSpeedBar[] = [
    { at: BAR_AT, output_speed_by_model: {
      "": { p50: null, p05: null }, all: { p50: 50, p05: 5 },
      ["__proto__"]: { p50: 10, p05: 2 },
    } },
    { at: "2026-10-30T10:00:00Z", output_speed_by_model: {} },
  ];
  it("defaults to a measured model, preserves explicit unmeasured choices and recovers when removed", async () => {
    const { rerender } = render(<OutputSpeedChart bars={bars} days={1} bucketSeconds={3600} />);
    expect(screen.getByRole("combobox")).toHaveTextContent("__proto__");
    for (const series of screen.getAllByTestId("series")) {
      expect(series.dataset.connected).toBe("false");
      expect(series.dataset.dot).toBe('{"r":2}');
    }
    expect(screen.getByTestId("axis-value")).toHaveTextContent("50 tokens/s");
    expect(JSON.parse(screen.getByTestId("lines").dataset.rows ?? "null")).toEqual([
      { at: BAR_AT, p50: 10, p05: 2 }, { at: bars[1]!.at, p50: null, p05: null },
    ]);
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("option", { name: "all" }));
    expect(screen.getByText(/latest median 50 tokens\/s, 5th percentile 5 tokens\/s/)).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("option", { name: "No model" }));
    rerender(<OutputSpeedChart bars={bars} days={1} bucketSeconds={3600} />);
    expect(screen.getByRole("combobox")).toHaveTextContent("No model");
    expect(screen.getByText(/No model call in the window reported its time to first token/)).toBeInTheDocument();
    rerender(<OutputSpeedChart bars={[HISTOGRAM.bars[0]!]} days={1} bucketSeconds={3600} />);
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(screen.getByText(/latest median 50 tokens\/s/)).toBeInTheDocument();
  });
  it("shows per-model p50 and p05 in the full charts and the strip", async () => {
    const { unmount } = render(<TraceCharts histogram={HISTOGRAM} marks={[]} />);
    expect(linesOf("Output speed")).toEqual([{ at: BAR_AT, p50: 50, p05: 5 }]);
    expect(within(screen.getByRole("region", { name: "Output speed" })).getAllByTestId("series").map((s) => s.textContent)).toEqual(["p50", "p05"]);
    unmount();
    render(<TraceHistogramStrip histogram={HISTOGRAM} marks={[]} />);
    fireEvent.keyDown(screen.getByRole("combobox", { name: "What the chart shows" }), { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("option", { name: "Output speed" }));
    expect(screen.getByText(/5th percentile 5 tokens\/s/)).toBeInTheDocument();
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
