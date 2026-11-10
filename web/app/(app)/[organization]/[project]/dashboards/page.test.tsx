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
// Recharts measures a box jsdom has none of: the charts are their bars' count.
vi.mock("./_charts", () => ({
  DashboardCharts: ({ bars, firstToken }: { bars: unknown[]; firstToken: unknown[] }) => (
    <div data-testid="charts" data-bars={bars.length} data-first-token={firstToken.length} />
  ),
}));
vi.mock("@/lib/server/project-page", () => ({
  projectPage: async () => ({
    kind: "ok",
    project: { id: "project_1", slug: "web", name: "Web", role: "member" },
  }),
}));
const fetchRollups = vi.hoisted(() => vi.fn());
vi.mock("@/lib/server/entities/dashboards", async (importActual) => ({
  ...(await importActual<typeof import("@/lib/server/entities/dashboards")>()),
  fetchRollups: (...args: unknown[]) => fetchRollups(...args),
}));
const fetchAlerts = vi.hoisted(() => vi.fn());
vi.mock("@/lib/server/entities/alerts", () => ({ fetchAlerts: (id: string) => fetchAlerts(id) }));
const fetchTraceHistogram = vi.hoisted(() => vi.fn());
vi.mock("@/lib/server/entities/traces", () => ({
  fetchTraceHistogram: (...args: unknown[]) => fetchTraceHistogram(...args),
}));
const histogram = () => ({
    kind: "ok",
    histogram: {
      bars: [
        { at: "2026-11-02T00:00:00Z", time_to_first_token_p50_ms: 300, time_to_first_token_p95_ms: 900 },
      ],
      bucket_seconds: 86_400,
    },
});

import DashboardsPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });

const point = (spend: number) => ({
  at: "2026-11-02T00:00:00Z",
  spans: 3,
  failed: 0,
  traces: 1,
  failed_traces: 0,
  input_tokens: 10,
  output_tokens: 5,
  cache_read_input_tokens: 0,
  cache_write_input_tokens: 0,
  spend,
  trace_duration_ms: { p50: 1, p95: 2, p99: 3 },
});

const rollups = (points: unknown[], truncated = false, days = 7) => ({
  kind: "ok",
  rollups: { days, step: "day", group: null, points, truncated },
});

beforeEach(() => {
  vi.clearAllMocks();
  fetchRollups.mockResolvedValue(rollups([point(5)]));
  fetchTraceHistogram.mockResolvedValue(histogram());
  fetchAlerts.mockResolvedValue({
    kind: "ok",
    alerts: [
      {
        id: "a1",
        name: "Monthly budget",
        kind: "agent",
        measure: "cost",
        aggregation: "sum",
        window: "month",
        alert_threshold: 100,
        value: 42.5,
        paused: null,
      },
      {
        id: "a3",
        name: "Unread budget",
        kind: "agent",
        measure: "cost",
        aggregation: "sum",
        window: "month",
        alert_threshold: 50,
        value: null,
        paused: null,
      },
      {
        id: "a4",
        name: "Paused budget",
        kind: "agent",
        measure: "cost",
        aggregation: "sum",
        window: "month",
        alert_threshold: 10,
        value: 3,
        paused: "person",
      },
      { id: "a2", name: "Slow", kind: "model_call", measure: "duration", aggregation: "p95", window: "hour", alert_threshold: 5, value: 1 },
    ],
  });
});

describe("DashboardsPage", () => {
  it("draws the window's charts and the month's budgets", async () => {
    render(await DashboardsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const charts = screen.getByTestId("charts");
    expect(charts.dataset.bars).toBe("1");
    expect(charts.dataset.firstToken).toBe("1");
    expect(fetchRollups).toHaveBeenCalledTimes(1);
    expect(fetchRollups).toHaveBeenCalledWith("project_1", 7);

    const budgets = within(screen.getByRole("region", { name: "Budgets this month" }));
    expect(budgets.getByText("Monthly budget")).toBeInTheDocument();
    expect(budgets.getByText("$42.50 of $100.00")).toBeInTheDocument();
    expect(budgets.getByText("Not read yet of $50.00")).toBeInTheDocument();
    expect(budgets.queryByText("Paused budget")).toBeNull();
    expect(budgets.queryByText("Slow")).toBeNull();
    expect(screen.queryByText("The window holds more than one read answers.")).toBeNull();
  });

  it("reads the window the URL asks for, and draws no first token where the spans could not be read", async () => {
    fetchTraceHistogram.mockResolvedValue({ kind: "unavailable" });
    render(await DashboardsPage({ params: PARAMS, searchParams: Promise.resolve({ days: "90" }) }));
    expect(fetchRollups).toHaveBeenCalledWith("project_1", 90);
    expect(screen.getByTestId("charts").dataset.firstToken).toBe("0");
  });

  it("says nothing has landed in the window the totals read, and draws no chart", async () => {
    fetchRollups.mockResolvedValue(rollups([], false, 30));
    render(await DashboardsPage({ params: PARAMS, searchParams: Promise.resolve({ days: "90" }) }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.getByText("No trace has landed in the last 30 days.")).toBeInTheDocument();
    expect(screen.queryByTestId("charts")).toBeNull();
  });

  it("says when a read left the window's oldest points out", async () => {
    fetchRollups.mockResolvedValue(rollups([point(5)], true));
    render(await DashboardsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("The window holds more than one read answers.")).toBeInTheDocument();
  });
});
