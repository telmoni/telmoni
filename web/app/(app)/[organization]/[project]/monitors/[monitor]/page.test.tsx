// @vitest-environment jsdom
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({
    title,
    backHref,
    action,
  }: {
    title: string;
    backHref?: string;
    action?: React.ReactNode;
  }) => (
    <div data-testid="page-header">
      <h1>{title}</h1>
      {backHref && <a href={backHref}>Monitors</a>}
      {action}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

// The strip measures the box it draws in, which jsdom has none of.
vi.mock("@/components/monitors/run-strip", () => ({
  RunStrip: ({ runs, expectedMs }: { runs: unknown[]; expectedMs: number | null }) => (
    <div data-testid="strip" data-runs={runs.length} data-expected={expectedMs ?? ""} />
  ),
}));

const pauseMonitorAction = vi.fn();
const resumeMonitorAction = vi.fn();
const deleteMonitorAction = vi.fn();
vi.mock("@/app/(app)/[organization]/[project]/monitors/actions", () => ({
  createMonitorAction: vi.fn(),
  updateMonitorAction: vi.fn(),
  previewScheduleAction: vi.fn(async () => ({ status: "ok", next: [] })),
  pauseMonitorAction: (...args: unknown[]) => pauseMonitorAction(...args),
  resumeMonitorAction: (...args: unknown[]) => resumeMonitorAction(...args),
  deleteMonitorAction: (...args: unknown[]) => deleteMonitorAction(...args),
}));

const push = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, refresh: vi.fn() }),
  usePathname: () => "/acme/web/monitors/nightly-report",
  useSearchParams: () => new URLSearchParams(),
  unstable_rethrow: () => {},
  notFound: () => {
    throw new Error("NEXT_NOT_FOUND");
  },
}));

import { MonitorState, Role } from "@/lib/types/enums";

let mockProject: unknown;
let mockMonitor: unknown;
let mockTraces: unknown;
let mockChanges: unknown;
const fetchMonitor = vi.fn<(projectId: string, slug: string) => Promise<unknown>>(
  async () => mockMonitor,
);
const fetchTraces = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(
  async () => mockTraces,
);
const fetchResourceAudit = vi.fn<
  (projectId: string, kind: string, id: string, limit: number) => Promise<unknown>
>(async () => mockChanges);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => ({ person: { userId: "user_1" }, organizations: [] }),
  fetchProjectBySlug: async () => mockProject,
  fetchMonitor: (projectId: string, slug: string) => fetchMonitor(projectId, slug),
  fetchTraces: (projectId: string, query: unknown) => fetchTraces(projectId, query),
  fetchTimezones: async () => ["Europe/London", "UTC"],
  fetchResourceAudit: (projectId: string, kind: string, id: string, limit: number) =>
    fetchResourceAudit(projectId, kind, id, limit),
}));

import MonitorPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web", monitor: "nightly-report" });
const PROJECT = "project_7bQx2mNv9BcK4dLp";
const ID = "0d2b7a6e-6c0f-4a1b-9f3e-1c2d3e4f5a6b";
const RUN = "5d4f5c68-0000-4000-8000-000000000001";

const MONITOR = {
  id: ID,
  slug: "nightly-report",
  name: "Nightly report",
  agent: "report-bot",
  schedule: "0 2 * * *",
  interval_s: null,
  timezone: "Europe/London",
  grace_s: 600,
  expected_duration_s: 300,
  bounds: { stall_after_s: 600, max_steps: 40, max_cost_usd: null },
  state: MonitorState.Running,
  state_changed_at: "2026-10-22T01:00:00Z",
  next_due_at: "2026-10-23T01:00:00Z",
  last_run: null,
  last_success_at: "2026-10-21T01:04:02Z",
  created_at: "2026-10-01T00:00:00Z",
  updated_at: "2026-10-22T01:00:00Z",
  open_runs: [
    {
      run_id: RUN,
      source: "ingest",
      started_at: "2026-10-22T01:00:00Z",
      last_heard_at: "2026-10-22T01:02:00Z",
      ends_by: "2026-10-22T01:20:00Z",
      steps: 4,
      input_tokens: 1200,
      output_tokens: 85,
      cost_usd: 0.25,
      bounds: { stall_after_s: 60, max_steps: 40, max_cost_usd: null },
    },
  ],
  incidents: [
    {
      id: "7a1c0000-0000-4000-8000-000000000002",
      cause: "stalled",
      detail: "run_stalled",
      opened_at: "2026-10-22T01:05:00Z",
      resolved_at: null,
      resolution: null,
      flapping: true,
    },
    {
      id: "7a1c0000-0000-4000-8000-000000000001",
      cause: "failed",
      detail: "run_failed",
      opened_at: "2026-10-20T01:04:00Z",
      resolved_at: "2026-10-21T01:04:00Z",
      resolution: "recovered",
      flapping: false,
    },
  ],
};

const TRACE = {
  run_id: RUN,
  agent: "report-bot",
  user_id: "",
  session_id: "",
  ended: true,
  status: "ok",
  error_type: "",
  started_at: "2026-10-21T01:00:00Z",
  ended_at: "2026-10-21T01:04:02Z",
  duration_ms: 242_000,
  spans: 4,
  steps: 3,
  model_calls: 1,
  tool_calls: 1,
  errors: 0,
  input_tokens: 1200,
  output_tokens: 300,
  cache_read_input_tokens: 0,
  cache_write_input_tokens: 0,
  models: [],
  tools: [],
  environments: [],
  releases: [],
};

beforeEach(() => {
  vi.clearAllMocks();
  window.localStorage.clear();
  mockProject = { id: PROJECT, slug: "web", name: "Web", role: Role.Member };
  mockMonitor = { kind: "ok", monitor: MONITOR };
  mockTraces = {
    kind: "ok",
    list: {
      traces: [TRACE],
      days: 90,
      limit: 50,
      page: 1,
      total: 1,
      more: false,
      sort: "started",
      dir: "desc",
    },
  };
  mockChanges = {
    kind: "ok",
    events: [
      {
        id: "e2",
        actor_id: "user_owner",
        action: "updated",
        resource_kind: "monitor",
        resource_id: ID,
        created_at: "2026-10-20T09:00:00Z",
        metadata: { slug: "nightly-report", changed: ["schedule", "grace_s"] },
      },
      {
        id: "e1",
        actor_id: "user_owner",
        action: "created",
        resource_kind: "monitor",
        resource_id: ID,
        created_at: "2026-10-01T00:00:00Z",
        metadata: { slug: "nightly-report", agent: "report-bot" },
      },
    ],
  };
});

function section(name: string) {
  return screen.getByRole("heading", { name }).closest("section")!;
}

describe("MonitorPage", () => {
  it("reads the monitor its path names, and shows its state, schedule and last success", async () => {
    render(await MonitorPage({ params: PARAMS }));
    expect(fetchMonitor).toHaveBeenCalledWith(PROJECT, "nightly-report");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Nightly report");
    expect(screen.getByRole("link", { name: "Monitors" })).toHaveAttribute("href", "/acme/web/monitors");
    expect(document.body).toHaveTextContent("Running");
    expect(document.body).toHaveTextContent("every day at 02:00, Europe/London");
    expect(document.body).toHaveTextContent("Next due");
    expect(document.body).toHaveTextContent("Last success");
  });

  it("shows the run going now against each bound it is held to", async () => {
    render(await MonitorPage({ params: PARAMS }));
    const running = section("Running now");
    expect(running).toHaveTextContent("Reported by its SDK");
    expect(running).toHaveTextContent("4 of 40");
    expect(running).toHaveTextContent("1,200 in · 85 out");
    expect(running).toHaveTextContent("$0.25");
    expect(running).toHaveTextContent("Quiet for up to 1 min");
  });

  it("gives each bound with where it came from, the tighter holding", async () => {
    render(await MonitorPage({ params: PARAMS }));
    const definition = section("Definition");
    expect(definition).toHaveTextContent("1 min, from its running run's code");
    expect(definition).toHaveTextContent("40 steps, from the monitor");
    expect(definition).toHaveTextContent("Cost capNone");
    expect(definition).toHaveTextContent("10 min");
    expect(definition).toHaveTextContent("5 min");
  });

  it("draws its last runs as a strip and as the trace list's table, its agent fixed", async () => {
    render(await MonitorPage({ params: PARAMS }));
    expect(fetchTraces).toHaveBeenCalledWith(PROJECT, { agent: ["report-bot"], days: 90, limit: 50 });
    const runs = section("Last runs");
    expect(within(runs).getByTestId("strip")).toHaveAttribute("data-runs", "1");
    expect(within(runs).getByTestId("strip")).toHaveAttribute("data-expected", "300000");
    expect(within(runs).getByRole("link", { name: "5d4f5c68" })).toBeInTheDocument();
    expect(within(runs).getByRole("link", { name: "Every trace of its agent" })).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=report-bot&days=90",
    );
  });

  it("lists its incidents, the open one first, and how long each resolved one stood", async () => {
    render(await MonitorPage({ params: PARAMS }));
    const incidents = within(section("Incidents"));
    const lines = incidents.getAllByText(/^A run /).map((n) => n.textContent);
    expect(lines).toEqual(["A run went quiet past its stall timeout", "A run ended in error"]);
    expect(incidents.getByText("flapping")).toBeInTheDocument();
    expect(incidents.getByText("Open")).toBeInTheDocument();
    expect(incidents.getByText("Stood 24 h")).toBeInTheDocument();
    expect(section("Incidents")).toHaveTextContent("Recovered");
  });

  it("offers a member no change to make, and reads no changes", async () => {
    render(await MonitorPage({ params: PARAMS }));
    for (const name of [/^Edit/, "Pause", "Resume", "Delete"]) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
    expect(fetchResourceAudit).not.toHaveBeenCalled();
    expect(screen.queryByRole("heading", { name: "Recent changes" })).toBeNull();
  });

  it("offers an owner Edit, Pause and Delete, and lists its recent changes", async () => {
    mockProject = { id: PROJECT, slug: "web", name: "Web", role: Role.Owner };
    pauseMonitorAction.mockResolvedValue({ status: "ok", monitor: MONITOR });
    render(await MonitorPage({ params: PARAMS }));
    expect(fetchResourceAudit).toHaveBeenCalledWith(PROJECT, "monitor", ID, 20);
    const changes = within(section("Recent changes"));
    expect(changes.getByText("Changed its schedule and grace period")).toBeInTheDocument();
    expect(changes.getByText("Created")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Edit/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    await vi.waitFor(() => expect(pauseMonitorAction).toHaveBeenCalledWith(PROJECT, "nightly-report"));
  });

  it("deletes only once its slug is typed, and goes back to the list", async () => {
    mockProject = { id: PROJECT, slug: "web", name: "Web", role: Role.Admin };
    deleteMonitorAction.mockResolvedValue({});
    render(await MonitorPage({ params: PARAMS }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    const confirm = within(dialog).getByRole("button", { name: "Delete monitor" });
    expect(confirm).toBeDisabled();
    fireEvent.change(within(dialog).getByRole("textbox"), { target: { value: "nightly-report" } });
    fireEvent.click(confirm);
    await vi.waitFor(() => expect(push).toHaveBeenCalledWith("/acme/web/monitors"));
    expect(deleteMonitorAction).toHaveBeenCalledWith(PROJECT, "nightly-report");
  });

  // Made by its first ping, a monitor is paused until set up: Edit starts it,
  // and a pause would change nothing.
  it("offers a new monitor no pause, and says Edit sets it up; a paused one, Resume", async () => {
    mockProject = { id: PROJECT, slug: "web", name: "Web", role: Role.Owner };
    mockMonitor = { kind: "ok", monitor: { ...MONITOR, state: MonitorState.New, open_runs: [] } };
    render(await MonitorPage({ params: PARAMS }));
    expect(screen.queryByRole("button", { name: "Pause" })).toBeNull();
    expect(document.body).toHaveTextContent("paused until it is set up");
    mockMonitor = { kind: "ok", monitor: { ...MONITOR, state: MonitorState.Paused, open_runs: [] } };
    render(await MonitorPage({ params: PARAMS }));
    expect(screen.getByRole("button", { name: "Resume" })).toBeInTheDocument();
  });

  it("is not found for a slug the project has no monitor by, and says when it cannot be read", async () => {
    mockMonitor = { kind: "missing" };
    await expect(MonitorPage({ params: PARAMS })).rejects.toThrow("NEXT_NOT_FOUND");
    mockMonitor = { kind: "unavailable" };
    render(await MonitorPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
