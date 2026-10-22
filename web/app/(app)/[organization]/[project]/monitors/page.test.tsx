// @vitest-environment jsdom
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({
    title,
    count,
    action,
  }: {
    title: string;
    count?: number;
    action?: React.ReactNode;
  }) => (
    <div data-testid="page-header">
      {title}
      {count !== undefined && <span data-testid="count">{count}</span>}
      {action}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

const createMonitorAction = vi.fn();
const previewScheduleAction = vi.fn();
vi.mock("@/app/(app)/[organization]/[project]/monitors/actions", () => ({
  createMonitorAction: (...args: unknown[]) => createMonitorAction(...args),
  updateMonitorAction: vi.fn(),
  previewScheduleAction: (...args: unknown[]) => previewScheduleAction(...args),
  mintPingKeyAction: vi.fn(),
  retirePingKeyAction: vi.fn(),
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/acme/web/monitors",
  useSearchParams: () => new URLSearchParams(),
  unstable_rethrow: () => {},
}));

import { MonitorState, Role } from "@/lib/types/enums";

let mockProject: unknown;
let mockMonitors: unknown;
const fetchMonitors = vi.fn<(projectId: string) => Promise<unknown>>(async () => mockMonitors);
const fetchTimezones = vi.fn<(projectId: string) => Promise<string[]>>(async () => [
  "Europe/London",
  "UTC",
]);
const fetchPingKeys = vi.fn<(projectId: string) => Promise<unknown>>(async () => ({
  kind: "ok",
  keys: [],
}));

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => ({ person: { userId: "user_1" }, organizations: [] }),
  fetchProjectBySlug: async () => mockProject,
  fetchMonitors: (projectId: string) => fetchMonitors(projectId),
  fetchTimezones: (projectId: string) => fetchTimezones(projectId),
  fetchPingKeys: (projectId: string) => fetchPingKeys(projectId),
}));

import MonitorsPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });
const PROJECT = "project_7bQx2mNv9BcK4dLp";

function monitor(over: Record<string, unknown>) {
  return {
    id: `m-${String(over.slug)}`,
    agent: over.slug,
    schedule: null,
    interval_s: null,
    timezone: "UTC",
    grace_s: null,
    expected_duration_s: null,
    bounds: { stall_after_s: null, max_steps: null, max_cost_usd: null },
    state: MonitorState.Healthy,
    state_changed_at: "2026-10-21T03:12:00Z",
    next_due_at: null,
    last_run: null,
    last_success_at: null,
    created_at: "2026-10-01T00:00:00Z",
    updated_at: "2026-10-21T03:12:00Z",
    ...over,
  };
}

const NIGHTLY = monitor({
  slug: "nightly-report",
  name: "Nightly report",
  agent: "report-bot",
  schedule: "0 2 * * *",
  timezone: "Europe/London",
  grace_s: 600,
  bounds: { stall_after_s: 600, max_steps: 40, max_cost_usd: null },
  state: MonitorState.Failed,
  next_due_at: "2026-10-22T01:00:00Z",
  last_run: {
    run_id: "5d4f5c68-0000-4000-8000-000000000001",
    started_at: "2026-10-21T01:00:00Z",
    ended_at: "2026-10-21T01:04:02Z",
    status: "error",
    steps: 12,
    cost_usd: 0.08,
  },
});
const HOURLY = monitor({
  slug: "hourly-sync",
  name: "Hourly sync",
  interval_s: 900,
  grace_s: 60,
  state: MonitorState.Late,
});
const FOUND = monitor({ slug: "found-by-ping", name: "found-by-ping", state: MonitorState.New });

async function renderPage(searchParams: Record<string, string | string[]> = {}) {
  render(await MonitorsPage({ params: PARAMS, searchParams: Promise.resolve(searchParams) }));
}

function rows() {
  return within(screen.getByRole("table", { name: "Monitors" }))
    .getAllByRole("row")
    .slice(1);
}

beforeAll(() => {
  // cmdk keeps the item in focus in view, which jsdom cannot scroll.
  Element.prototype.scrollIntoView = vi.fn();
});

beforeEach(() => {
  vi.clearAllMocks();
  mockProject = { id: PROJECT, slug: "web", name: "Web", role: Role.Member };
  mockMonitors = { kind: "ok", monitors: [NIGHTLY, HOURLY, FOUND] };
  previewScheduleAction.mockResolvedValue({ status: "ok", next: [] });
});

describe("MonitorsPage", () => {
  it("lists every monitor in the lane's order, each in words, its name opening its page", async () => {
    await renderPage();
    expect(fetchMonitors).toHaveBeenCalledWith(PROJECT);
    expect(screen.getByTestId("count")).toHaveTextContent("3");
    const [nightly, hourly, found] = rows();
    expect(within(nightly!).getByRole("link", { name: "Nightly report" })).toHaveAttribute(
      "href",
      "/acme/web/monitors/nightly-report",
    );
    expect(nightly).toHaveTextContent("report-bot");
    expect(nightly).toHaveTextContent("every day at 02:00, Europe/London");
    expect(nightly).toHaveTextContent("Failed");
    expect(nightly).toHaveTextContent("Error · 4 min 2 s · 12 / 40 steps · $0.08");
    expect(nightly).toHaveTextContent("stall 10 min · 40 steps");
    expect(hourly).toHaveTextContent("every 15 minutes");
    expect(hourly).toHaveTextContent("Late");
    expect(hourly).toHaveTextContent("None yet");
    expect(found).toHaveTextContent("no schedule");
    expect(found).toHaveTextContent("New");
  });

  it("narrows to the states the URL picks and the words it carries", async () => {
    await renderPage({ state: ["late", "new"], q: "SYNC" });
    expect(rows().map((r) => within(r).getAllByRole("cell")[0]!.textContent)).toEqual([
      "Hourly sync",
    ]);
    expect(screen.getByTestId("count")).toHaveTextContent("3");
    expect(screen.getByRole("link", { name: "Clear the state late" })).toHaveAttribute(
      "href",
      "/acme/web/monitors?state=new&q=SYNC",
    );
    expect(screen.getByRole("link", { name: "Clear all" })).toHaveAttribute(
      "href",
      "/acme/web/monitors",
    );
  });

  it("says when no monitor matches, and how to see them all", async () => {
    await renderPage({ q: "nothing like it" });
    expect(screen.getByText("No monitors match.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Clear the filters" })).toHaveAttribute(
      "href",
      "/acme/web/monitors",
    );
  });

  it("says what a monitor is while the project has none", async () => {
    mockMonitors = { kind: "ok", monitors: [] };
    await renderPage();
    expect(screen.getByText("No monitors yet.")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByRole("search")).toBeNull();
  });

  // A member reads the list and makes nothing: no button, and neither the
  // zones nor the keys are asked for.
  it("offers a member nothing to make, and asks for no key", async () => {
    await renderPage();
    expect(screen.queryByRole("button", { name: /New monitor/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Ping key/ })).toBeNull();
    expect(fetchTimezones).not.toHaveBeenCalled();
    expect(fetchPingKeys).not.toHaveBeenCalled();
  });

  it("says when the monitors cannot be read, or may not be", async () => {
    mockMonitors = { kind: "unavailable" };
    await renderPage();
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    mockMonitors = { kind: "forbidden" };
    await renderPage();
    expect(screen.getByText(/this project's monitors/)).toBeInTheDocument();
  });

  // An owner makes one: the slug follows the name until typed, the lane is
  // sent what the form holds, and the panel then says how its runs report.
  it("lets an owner make a monitor, and says how its runs are reported", async () => {
    mockProject = { id: PROJECT, slug: "web", name: "Web", role: Role.Owner };
    createMonitorAction.mockResolvedValue({
      status: "ok",
      monitor: { ...NIGHTLY, slug: "weekly-digest", name: "Weekly digest", agent: "weekly-digest" },
    });
    await renderPage();
    expect(fetchTimezones).toHaveBeenCalledWith(PROJECT);
    expect(fetchPingKeys).toHaveBeenCalledWith(PROJECT);
    expect(screen.getByRole("button", { name: /Ping key/ })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /New monitor/ }));
    const panel = screen.getByRole("dialog", { name: "New monitor" });
    fireEvent.change(within(panel).getByLabelText("Name"), { target: { value: "Weekly digest" } });
    expect(within(panel).getByLabelText("Slug")).toHaveValue("weekly-digest");
    fireEvent.change(within(panel).getByLabelText("Cron line"), {
      target: { value: "0 9 * * 1" },
    });
    fireEvent.click(within(panel).getByRole("button", { name: "Create monitor" }));

    expect(await screen.findByRole("dialog", { name: "Weekly digest is watching" })).toBeInTheDocument();
    expect(createMonitorAction).toHaveBeenCalledWith(PROJECT, {
      name: "Weekly digest",
      slug: "weekly-digest",
      schedule: "0 9 * * 1",
      timezone: expect.any(String),
      grace_s: 600,
      bounds: {},
    });
    const report = screen.getByRole("dialog", { name: "Weekly digest is watching" });
    expect(report).toHaveTextContent(
      'curl -fsS -m 10 "$TELMONI_ENDPOINT/ping/v1/$TELMONI_PING_KEY/weekly-digest/start"',
    );
    expect(report).toHaveTextContent("telmoni run --monitor weekly-digest -- <command>");
    expect(report).not.toHaveTextContent("telmoniping_");
  });

  it("says which field the lane refused, and keeps the panel open", async () => {
    mockProject = { id: PROJECT, slug: "web", name: "Web", role: Role.Admin };
    createMonitorAction.mockResolvedValue({
      status: "error",
      error: "Bad Request: a monitor's grace_s is whole seconds",
    });
    await renderPage();
    fireEvent.click(screen.getByRole("button", { name: /New monitor/ }));
    const panel = screen.getByRole("dialog", { name: "New monitor" });
    fireEvent.change(within(panel).getByLabelText("Name"), { target: { value: "Report" } });
    fireEvent.change(within(panel).getByLabelText("Cron line"), { target: { value: " " } });
    fireEvent.click(within(panel).getByRole("button", { name: "Create monitor" }));
    expect(await within(panel).findByRole("alert")).toHaveTextContent("Write the cron line");
    expect(within(panel).getByLabelText("Cron line")).toHaveAttribute("aria-invalid", "true");
    expect(createMonitorAction).not.toHaveBeenCalled();
    fireEvent.change(within(panel).getByLabelText("Cron line"), { target: { value: "0 2 * * *" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Create monitor" }));
    expect(await within(panel).findByText(/grace_s/)).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "New monitor" })).toBeInTheDocument();
  });
});
