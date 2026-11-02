// @vitest-environment jsdom
import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title?: string; action?: React.ReactNode }) => (
    <div data-testid="page-header">
      <h1>{title}</h1>
      {action}
    </div>
  ),
}));
vi.mock("@/components/window-picker", () => ({ WindowPicker: () => null }));
vi.mock("./_group-picker", () => ({ GroupPicker: () => null }));
// Recharts measures a box jsdom has none of: the charts are their bars' count.
vi.mock("./_charts", () => ({
  DashboardCharts: ({ bars, firstToken }: { bars: unknown[]; firstToken: unknown[] }) => (
    <div data-testid="charts" data-bars={bars.length} data-first-token={firstToken.length} />
  ),
}));
vi.mock("@/lib/server/project-page", () => ({
  projectPage: async () => ({ kind: "ok", project: { id: "project_1", slug: "web", name: "Web" } }),
}));

const fetchRollups = vi.hoisted(() => vi.fn());
vi.mock("@/lib/server/entities/dashboards", async (importActual) => ({
  ...(await importActual<typeof import("@/lib/server/entities/dashboards")>()),
  fetchRollups: (...args: unknown[]) => fetchRollups(...args),
}));
const fetchAlerts = vi.hoisted(() => vi.fn());
vi.mock("@/lib/server/entities/alerts", () => ({ fetchAlerts: (id: string) => fetchAlerts(id) }));
vi.mock("@/lib/server/entities/traces", () => ({
  fetchTraceHistogram: async () => ({
    kind: "ok",
    histogram: {
      bars: [
        { at: "2026-11-02T00:00:00Z", time_to_first_token_p50_ms: 300, time_to_first_token_p95_ms: 900 },
      ],
    },
  }),
}));

import DashboardsPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });

const point = (value: string | undefined, spend: number) => ({
  at: "2026-11-02T00:00:00Z",
  value,
  spans: 3,
  failed: 0,
  runs: 1,
  failed_runs: 0,
  input_tokens: 10,
  output_tokens: 5,
  cache_read_input_tokens: 0,
  cache_write_input_tokens: 0,
  spend,
  run_duration_ms: { p50: 1, p95: 2, p99: 3 },
});

const rollups = (points: unknown[], group: string | null = null) => ({
  kind: "ok",
  rollups: { days: 7, step: "day", group, points, truncated: false },
});

beforeEach(() => {
  vi.clearAllMocks();
  fetchRollups.mockImplementation(async (_id: string, _days: number, group: string | null) =>
    group ? rollups([point("gpt-5", 1), point("claude", 4)], group) : rollups([point(undefined, 5)]),
  );
  fetchAlerts.mockResolvedValue({
    kind: "ok",
    alerts: [
      {
        id: "a1",
        name: "Monthly budget",
        kind: "run",
        measure: "cost",
        aggregation: "sum",
        window: "month",
        alert_threshold: 100,
        value: 42.5,
      },
      { id: "a2", name: "Slow", kind: "model_call", measure: "duration", aggregation: "p95", window: "hour", alert_threshold: 5, value: 1 },
    ],
  });
});

describe("DashboardsPage", () => {
  it("draws the window's charts, the month's budgets and each group's totals, the costliest first", async () => {
    render(await DashboardsPage({ params: PARAMS, searchParams: Promise.resolve({ group: "model" }) }));
    const charts = screen.getByTestId("charts");
    expect(charts.dataset.bars).toBe("1");
    expect(charts.dataset.firstToken).toBe("1");
    expect(fetchRollups).toHaveBeenCalledWith("project_1", 7, null);
    expect(fetchRollups).toHaveBeenCalledWith("project_1", 7, "model");

    const budgets = within(screen.getByRole("region", { name: "Budgets" }));
    expect(budgets.getByText("Monthly budget")).toBeInTheDocument();
    expect(budgets.getByText("$42.50 of $100.00")).toBeInTheDocument();
    expect(budgets.queryByText("Slow")).toBeNull();

    const byModel = screen.getByRole("region", { name: "By group" });
    expect(within(byModel).getAllByText(/^\$/).map((cell) => cell.textContent)).toEqual(["$4.00", "$1.00"]);
  });

  it("says nothing has landed in an empty window, and draws no chart", async () => {
    fetchRollups.mockResolvedValue(rollups([]));
    render(await DashboardsPage({ params: PARAMS, searchParams: Promise.resolve({ days: "30" }) }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.getByText("No run has landed in the last 30 days.")).toBeInTheDocument();
    expect(screen.queryByTestId("charts")).toBeNull();
  });
});
