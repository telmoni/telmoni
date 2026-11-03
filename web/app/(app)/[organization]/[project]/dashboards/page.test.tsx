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
vi.mock("./_charts-picker", () => ({
  ChartsPicker: ({ tagKeys }: { tagKeys: string[] }) => <div data-testid="picker" data-tags={tagKeys.join()} />,
}));
const menu = vi.hoisted(() => vi.fn());
vi.mock("./_dashboards-menu", () => ({
  DashboardsMenu: (props: unknown) => {
    menu(props);
    return null;
  },
}));
// Recharts measures a box jsdom has none of: the charts are their words and
// their bars' count, and each split its read.
vi.mock("./_charts", () => ({
  DashboardCharts: ({
    charts,
    bars,
    firstToken,
    splits,
  }: {
    charts: { measure: string; group?: string; tag?: string }[];
    bars: unknown[];
    firstToken: unknown[];
    splits: Record<string, { kind: string; days?: number }>;
  }) => (
    <div
      data-testid="charts"
      data-charts={charts.map((c) => [c.measure, c.group, c.tag].filter(Boolean).join(":")).join()}
      data-bars={bars.length}
      data-first-token={firstToken.length}
      data-splits={JSON.stringify(splits, (key, value) => (key === "points" ? value.length : value))}
    />
  ),
}));
vi.mock("@/lib/server/project-page", () => ({
  projectPage: async () => ({
    kind: "ok",
    project: { id: "project_1", slug: "web", name: "Web", role: "member" },
  }),
}));
const fetchDashboards = vi.hoisted(() => vi.fn());

const fetchRollups = vi.hoisted(() => vi.fn());
vi.mock("@/lib/server/entities/dashboards", async (importActual) => ({
  ...(await importActual<typeof import("@/lib/server/entities/dashboards")>()),
  fetchRollups: (...args: unknown[]) => fetchRollups(...args),
  fetchDashboards: (id: string) => fetchDashboards(id),
}));
const fetchAlerts = vi.hoisted(() => vi.fn());
vi.mock("@/lib/server/entities/alerts", () => ({ fetchAlerts: (id: string) => fetchAlerts(id) }));
const fetchTraceHistogram = vi.hoisted(() => vi.fn());
vi.mock("@/lib/server/entities/traces", () => ({
  fetchTraceFacets: async () => ({
    kind: "ok",
    facets: { tag: { values: [{ value: "team:billing", count: 2 }, { value: "repo:core", count: 1 }], more: false } },
  }),
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

const rollups = (points: unknown[], group: string | null = null, truncated = false, days = 7) => ({
  kind: "ok",
  rollups: { days, step: "day", group, points, truncated },
});

beforeEach(() => {
  vi.clearAllMocks();
  fetchRollups.mockImplementation(async (_id: string, days: number, group?: string) =>
    group
      ? rollups([point("gpt-5", 1), point("claude", 4)], group, false, group === "user" ? 30 : days)
      : rollups([point(undefined, 5)]),
  );
  fetchTraceHistogram.mockResolvedValue(histogram());
  fetchDashboards.mockResolvedValue({
    kind: "ok",
    dashboards: [
      {
        id: "d-1",
        kind: "dashboard",
        name: "Team spend",
        query: { charts: [{ measure: "spend", group: "tag", tag: "team" }, { measure: "spend", group: "tool" }] },
        columns: [],
        created_at: "2026-11-02T00:00:00Z",
        updated_at: "2026-11-02T00:00:00Z",
      },
    ],
  });
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
        paused: null,
      },
      {
        id: "a3",
        name: "Unread budget",
        kind: "run",
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
        kind: "run",
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
  it("opens on the cost dashboard and the month's budgets", async () => {
    render(await DashboardsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const charts = screen.getByTestId("charts");
    expect(charts.dataset.charts).toBe("spend,tokens,runs,latency,first_token");
    expect(charts.dataset.bars).toBe("1");
    expect(charts.dataset.firstToken).toBe("1");
    expect(fetchRollups).toHaveBeenCalledTimes(1);
    expect(fetchRollups).toHaveBeenCalledWith("project_1", 7);
    expect(screen.getByTestId("picker").dataset.tags).toBe("repo,team");

    const budgets = within(screen.getByRole("region", { name: "Budgets this month" }));
    expect(budgets.getByText("Monthly budget")).toBeInTheDocument();
    expect(budgets.getByText("$42.50 of $100.00")).toBeInTheDocument();
    expect(budgets.getByText("Not read yet of $50.00")).toBeInTheDocument();
    expect(budgets.queryByText("Paused budget")).toBeNull();
    expect(budgets.queryByText("Slow")).toBeNull();
    expect(screen.queryByText("The window holds more than one read answers.")).toBeNull();
  });

  it("draws the URL's charts, reading each split once, and the spans' none past their window", async () => {
    render(
      await DashboardsPage({
        params: PARAMS,
        searchParams: Promise.resolve({ days: "90", chart: ["spend:user", "spend:model", "spend:user", "runs"] }),
      }),
    );
    const charts = screen.getByTestId("charts");
    expect(charts.dataset.charts).toBe("spend:user,spend:model,runs");
    expect(JSON.parse(charts.dataset.splits ?? "")).toEqual({
      "spend:user": { kind: "ok", points: 2, days: 30 },
      "spend:model": { kind: "ok", points: 2, days: 90 },
    });
    expect(fetchRollups).toHaveBeenCalledWith("project_1", 90, "user", undefined);
    expect(fetchRollups).toHaveBeenCalledWith("project_1", 90, "model", undefined);
    expect(fetchTraceHistogram).not.toHaveBeenCalled();
  });

  it("opens a saved dashboard on its charts, leaving out one the page does not draw", async () => {
    render(await DashboardsPage({ params: PARAMS, searchParams: Promise.resolve({ view: "d-1" }) }));
    expect(screen.getByTestId("charts").dataset.charts).toBe("spend:tag:team");
    expect(fetchRollups).toHaveBeenCalledWith("project_1", 7, "tag", "team");
    const props = menu.mock.calls[0]?.[0] as { current?: { id: string }; canSave: boolean };
    expect(props.current?.id).toBe("d-1");
    expect(props.canSave).toBe(false);
  });

  it("names a split that could not be read as unread, and draws the rest", async () => {
    fetchRollups.mockImplementation(async (_id: string, _days: number, group?: string) =>
      group ? { kind: "unavailable" } : rollups([point(undefined, 5)]),
    );
    render(await DashboardsPage({ params: PARAMS, searchParams: Promise.resolve({ chart: ["spend:session"] }) }));
    expect(JSON.parse(screen.getByTestId("charts").dataset.splits ?? "")).toEqual({
      "spend:session": { kind: "unread" },
    });
  });

  it("says nothing has landed in the window the totals read, and draws no chart", async () => {
    fetchRollups.mockResolvedValue(rollups([], null, false, 30));
    render(await DashboardsPage({ params: PARAMS, searchParams: Promise.resolve({ days: "90" }) }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.getByText("No run has landed in the last 30 days.")).toBeInTheDocument();
    expect(screen.queryByTestId("charts")).toBeNull();
  });

  it("says when a read left the window's oldest points out", async () => {
    fetchRollups.mockResolvedValue(rollups([point(undefined, 5)], null, true));
    render(await DashboardsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("The window holds more than one read answers.")).toBeInTheDocument();
  });
});
