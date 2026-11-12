// @vitest-environment jsdom
import { fireEvent, render, screen, within } from "@testing-library/react";
import type { MouseEventHandler, ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

// Recharts measures the box it draws in, which jsdom has none of: each chart
// is drawn here as the rows it was handed and the series it draws of them,
// with the plot's grid standing where a click is read against it.
vi.mock("recharts", () => {
  const chart = (testid: string) =>
    function Chart({ data, children }: { data: unknown; children: ReactNode }) {
      return (
        <div data-testid={testid} data-rows={JSON.stringify(data)}>
          {children}
        </div>
      );
    };
  return {
    LineChart: chart("lines"),
    BarChart: chart("bars"),
    Line: ({ dataKey, type, connectNulls }: { dataKey: string; type?: string; connectNulls?: boolean }) => (
      <span data-testid="series" data-type={type} data-connected={connectNulls}>
        {dataKey}
      </span>
    ),
    Bar: ({ dataKey, children }: { dataKey: string; children?: ReactNode }) => (
      <span data-testid="series">
        {dataKey}
        {children}
      </span>
    ),
    Cell: ({ fill, strokeDasharray }: { fill?: string; strokeDasharray?: string }) => (
      <i data-testid="cell" data-fill={fill} data-dashed={strokeDasharray} />
    ),
    ReferenceLine: () => null,
    CartesianGrid: () => <span className="recharts-cartesian-grid" />,
    XAxis: () => null,
    YAxis: () => null,
  };
});

// The tooltip as recharts draws it over the second step: its label and its rows.
vi.mock("@/components/ui/chart", () => ({
  ChartContainer: ({ children, onClick }: { children: ReactNode; onClick?: MouseEventHandler<HTMLDivElement> }) => (
    <div data-testid="plot" onClick={onClick}>
      {children}
    </div>
  ),
  ChartLegend: () => null,
  ChartLegendContent: () => null,
  ChartTooltip: ({ content }: { content: ReactNode }) => content,
  ChartTooltipContent: ({
    labelFormatter,
    formatter,
  }: {
    labelFormatter?: (label: unknown, payload: { payload: { id: string } }[]) => ReactNode;
    formatter?: (value: unknown, name: string, item: { payload: { id: string } }) => ReactNode;
  }) => (
    <div data-testid="tooltip">
      <b>{labelFormatter?.(undefined, [{ payload: { id: SECOND } }])}</b>
      {formatter?.(0.75, "spent", { payload: { id: SECOND } })}
    </div>
  ),
}));

const { openPeek } = vi.hoisted(() => ({ openPeek: vi.fn() }));
vi.mock("@/lib/peek", () => ({ openPeek }));

import { SessionCharts } from "@/components/traces/charts";

const FIRST = "0191a3b47c2e7d3f8e4a5b6c7d8e9f01";
const SECOND = "0291a3b47c2e7d3f8e4a5b6c7d8e9f02";
const THIRD = "0391a3b47c2e7d3f8e4a5b6c7d8e9f03";

const trace = (trace_id: string, started_at: string, patch: Record<string, unknown> = {}) => ({
  trace_id,
  started_at,
  ended: true,
  status: "ok" as const,
  duration_ms: 1_000,
  spend: 0.25,
  time_to_first_token_ms: null as number | null,
  input_tokens: 1_000,
  output_tokens: 200,
  cache_read_input_tokens: 400,
  cache_write_input_tokens: 100,
  reasoning_output_tokens: 50,
  ...patch,
});

const TRACES = [
  trace(FIRST, "2026-10-30T09:00:00Z", { time_to_first_token_ms: 350 }),
  trace(SECOND, "2026-10-30T09:05:00Z", { status: "error" as const, spend: 0.5, duration_ms: 4_000 }),
  // Its agent's own span has not arrived: running, whatever its spans say so far.
  trace(THIRD, "2026-10-30T09:20:00Z", { ended: false, spend: 0.125 }),
];

const rowsOf = (region: string, testid: string) =>
  JSON.parse(within(screen.getByRole("region", { name: region })).getByTestId(testid).dataset.rows ?? "[]") as Record<
    string,
    unknown
  >[];

describe("a session's charts", () => {
  beforeEach(() => vi.clearAllMocks());

  it("draws a step a trace in the order given, each with what it took and what the session had cost by then", () => {
    render(<SessionCharts traces={TRACES} />);
    const steps = rowsOf("Duration", "bars");
    expect(steps.map((step) => [step.id, step.state, step.duration])).toEqual([
      [FIRST, "ok", 1_000],
      [SECOND, "error", 4_000],
      [THIRD, "running", 1_000],
    ]);
    // The cost so far adds up along the session; a trace's own stays beside it.
    expect(rowsOf("Cost so far", "lines").map((step) => [step.cost, step.spent])).toEqual([
      [0.25, 0.25],
      [0.5, 0.75],
      [0.125, 0.875],
    ]);
    // The tokens as five that add up to input and output, as every tokens chart draws them.
    expect(rowsOf("Tokens", "bars")[0]).toMatchObject({
      input: 500,
      cacheRead: 400,
      cacheWrite: 100,
      output: 150,
      reasoning: 50,
    });
    expect(rowsOf("Time to first token", "lines").map((step) => step.first_token)).toEqual([350, null, null]);
  });

  it("draws the cost as steps and a time to first token as dots, a trace that reported none a gap", () => {
    render(<SessionCharts traces={TRACES} />);
    const cost = within(screen.getByRole("region", { name: "Cost so far" })).getByTestId("series");
    expect(cost).toHaveTextContent("spent");
    expect(cost).toHaveAttribute("data-type", "stepAfter");
    const first = within(screen.getByRole("region", { name: "Time to first token" })).getByTestId("series");
    expect(first).toHaveAttribute("data-connected", "false");
    expect(screen.getByText("How soon each trace's first model call began to answer.")).toBeInTheDocument();
  });

  it("colours a bar by how its trace stands, one still running hollow, and names only the states it holds", () => {
    render(<SessionCharts traces={TRACES} />);
    const duration = within(screen.getByRole("region", { name: "Duration" }));
    expect(duration.getAllByTestId("cell").map((cell) => [cell.dataset.fill, cell.dataset.dashed])).toEqual([
      ["var(--muted-foreground)", undefined],
      ["var(--destructive)", undefined],
      ["transparent", "3 2"],
    ]);
    const legend = within(duration.getByRole("list", { name: "How its traces stand" }));
    expect(legend.getAllByRole("listitem").map((item) => item.textContent)).toEqual(["OK", "Error", "Running"]);
  });

  it("heads a step's tooltip with when its trace started and its id, and says its cost beside the session's", () => {
    render(<SessionCharts traces={TRACES} />);
    const tooltip = within(screen.getByRole("region", { name: "Cost so far" })).getByTestId("tooltip");
    expect(tooltip.querySelector("b")).toHaveTextContent(/· 0291a3b4$/);
    expect(tooltip).toHaveTextContent("This trace$0.50");
    expect(tooltip).toHaveTextContent("So far$0.75");
    // On the duration chart the same step says how its trace stands.
    const stands = within(screen.getByRole("region", { name: "Duration" })).getByTestId("tooltip");
    expect(stands).toHaveTextContent("Error");
  });

  // ⚠ Read off where the click landed, never off the step recharts holds
  // hovered: a tap on a touch screen leaves that one tap behind.
  it("opens the trace whose band a click or a tap lands in, on any of the four charts", () => {
    // The plot as a browser lays it out: 300 px wide from 100, so three bands of 100.
    const box = vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      const plot = this.classList.contains("recharts-cartesian-grid");
      return (plot ? { left: 100, width: 300, top: 10, bottom: 210 } : { left: 0, width: 0, top: 0, bottom: 0 }) as DOMRect;
    });
    try {
      render(<SessionCharts traces={TRACES} />);
      const plots = screen.getAllByTestId("plot");
      expect(plots).toHaveLength(4);
      for (const plot of plots) fireEvent.click(plot, { clientX: 250, clientY: 100 });
      expect(openPeek.mock.calls).toEqual([[SECOND], [SECOND], [SECOND], [SECOND]]);
      openPeek.mockClear();
      // Its axis to the left, past its right edge, and its legend below open nothing.
      for (const at of [
        { clientX: 60, clientY: 100 },
        { clientX: 401, clientY: 100 },
        { clientX: 250, clientY: 230 },
      ]) {
        fireEvent.click(plots[0]!, at);
      }
      expect(openPeek).not.toHaveBeenCalled();
      fireEvent.click(plots[0]!, { clientX: 399, clientY: 100 });
      expect(openPeek).toHaveBeenCalledExactlyOnceWith(THIRD);
    } finally {
      box.mockRestore();
    }
  });

  it("says so where no trace reported a time to first token", () => {
    render(<SessionCharts traces={[trace(FIRST, "2026-10-30T09:00:00Z")]} />);
    expect(screen.getByText("No trace of the session reported its time to first token.")).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "Duration" })).getAllByRole("listitem")).toHaveLength(1);
  });
});
