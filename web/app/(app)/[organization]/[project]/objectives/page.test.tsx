// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { Role } from "@/lib/types/enums";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title: string; action?: React.ReactNode }) => (
    <div data-testid="page-header">
      {title}
      {action}
    </div>
  ),
}));
vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));
vi.mock("./_objective-panel", () => ({ NewObjective: () => <button type="button">New objective</button> }));
vi.mock("../alerts/_alert-actions", () => ({
  AlertActions: ({ alert }: { alert: { name: string } }) => <button type="button">Edit {alert.name}</button>,
}));
vi.mock("@/lib/server/project-page", () => ({
  projectPage: async () => ({
    kind: "ok",
    gate: {},
    project: { id: "project_7bQx2mNv9BcK4dLp", slug: "web", name: "Web", role: Role.Member },
  }),
}));

const BURN = {
  id: "7a1c0e52-0000-4000-8000-000000000009",
  name: "Support answers",
  kind: "run",
  measure: "burn",
  aggregation: "avg",
  filters: { agent: ["support-bot"] },
  window: "hour",
  operator: "gte",
  alert_threshold: 14.4,
  warning_threshold: 6,
  missing_data: "zero",
  renotify_minutes: null,
  score_name: null,
  baseline: "none",
  objective: { target: 0.99, latency_ms: null, days: 30 },
  paused: null,
  paused_at: null,
  value: 15,
  evaluated_at: null,
  severity: "alert",
  severity_since: "2026-11-04T00:00:00Z",
  connectors: [],
  created_at: "2026-11-01T00:00:00Z",
  updated_at: "2026-11-01T00:00:00Z",
};
let mockAlerts: unknown[];
let mockStandings: unknown;
vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  fetchAlerts: async () => ({ kind: "ok", alerts: mockAlerts, incidents: [], mutes: [], at: 0 }),
  fetchObjectives: async () => mockStandings,
  fetchConnectors: async () => ({ connections: [] }),
  fetchHistory: async () => null,
}));

import ObjectivesPage from "./page";

const page = async () =>
  render(
    await ObjectivesPage({
      params: Promise.resolve({ organization: "acme", project: "web" }),
      searchParams: Promise.resolve({}),
    }),
  );

beforeEach(() => {
  mockAlerts = [BURN, { ...BURN, id: "other", name: "Slow calls", measure: "duration", objective: undefined }];
  mockStandings = [
    {
      rule_id: BURN.id,
      hour_burn: 15,
      six_hour_burn: 4.25,
      days: 30,
      runs: 10_000,
      missed: 25,
      budget_left: 0.75,
    },
  ];
});

describe("ObjectivesPage", () => {
  it("lists the objectives alone, each with its promise, budget left and burns", async () => {
    await page();
    expect(screen.getByRole("button", { name: "New objective" })).toBeInTheDocument();
    expect(screen.queryByText("Slow calls")).toBeNull();
    const row = screen.getByRole("row", { name: /Support answers/ });
    expect(row).toHaveTextContent("99% of support-bot's runs succeed over 30 days");
    expect(row).toHaveTextContent("75%");
    expect(row).toHaveTextContent("25 of 10,000 runs missed, 30 days");
    expect(row).toHaveTextContent("15×");
    expect(row).toHaveTextContent("4.25×");
    expect(row).toHaveTextContent("Gone in 2.0 days at this rate");
    expect(screen.getByRole("link", { name: "Its incidents" })).toHaveAttribute(
      "href",
      `/acme/web/alerts#alert-${BURN.id}`,
    );
  });

  it("says when there is none, and when where they stand could not be read", async () => {
    mockAlerts = [];
    const { unmount } = await page();
    expect(screen.getByText("No objective yet.")).toBeInTheDocument();
    unmount();
    mockAlerts = [BURN];
    mockStandings = null;
    await page();
    expect(screen.getByText("Where they stand could not be read.")).toBeInTheDocument();
  });
});
