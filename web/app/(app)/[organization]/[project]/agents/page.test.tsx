// @vitest-environment jsdom
import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({
    title,
    action,
    backHref,
    backLabel,
    visitHref,
  }: {
    title?: string;
    action?: React.ReactNode;
    backHref?: string;
    backLabel?: string;
    visitHref?: string | null;
  }) => (
    <div data-testid="page-header" data-visit={visitHref === null ? "none" : visitHref}>
      <a href={backHref}>{backLabel}</a>
      <h1>{title}</h1>
      {action}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

// The charts measure the box they draw in, which jsdom has none of.
vi.mock("@/components/traces/charts", () => ({
  TraceCharts: ({ marks, models }: { marks: unknown; models: unknown }) => (
    <div data-testid="charts" data-marks={JSON.stringify(marks)} data-models={JSON.stringify(models)} />
  ),
}));

// The peek reads a trace once opened, and is proved by its own suite.
vi.mock("@/components/traces/peek", () => ({
  TracePeek: ({ traces, me }: { traces: readonly string[]; me: string }) => (
    <div data-testid="peek" data-traces={traces.join(" ")} data-me={me} />
  ),
}));

// The versions read once drawn, and are proved by their own suite.
vi.mock("../_versions-panel", () => ({
  AgentVersions: ({ projectId, agent, days }: { projectId: string; agent: string; days: number }) => (
    <div data-testid="versions" data-project={projectId} data-agent={agent} data-days={days} />
  ),
}));

class Redirected extends Error {}
vi.mock("next/navigation", () => ({
  redirect: (to: string) => {
    throw new Redirected(to);
  },
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/acme/web/agents",
  useSearchParams: () => new URLSearchParams("agent=support-bot"),
}));

let mockOffset: string | undefined;
vi.mock("next/headers", () => ({
  cookies: async () => ({
    get: (name: string) =>
      name === "telmoni-utc-offset" && mockOffset !== undefined ? { value: mockOffset } : undefined,
  }),
}));

let mockProject: { id: string; slug: string; name: string; role: string } | null;
let mockHistogram: unknown;
let mockFailed: unknown;
const fetchTraceHistogram = vi.fn<(projectId: string, query: unknown, utcOffset: number) => Promise<unknown>>(
  async () => mockHistogram,
);
const fetchTraces = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(async () => mockFailed);
vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => ({ person: { userId: "user_1" } }),
  fetchProjectBySlug: async () => mockProject,
  fetchTraceHistogram: (projectId: string, query: unknown, utcOffset: number) =>
    fetchTraceHistogram(projectId, query, utcOffset),
  fetchTraces: (projectId: string, query: unknown) => fetchTraces(projectId, query),
}));
vi.mock("@/lib/server/entities/content-mode", () => ({ fetchContentMode: async () => null }));

let mockAgents: unknown;
const fetchAgents = vi.fn<(projectId: string, agent?: string, days?: number) => Promise<unknown>>(
  async () => mockAgents,
);
vi.mock("@/lib/server/entities/agents", () => ({
  fetchAgents: (projectId: string, agent?: string, days?: number) => fetchAgents(projectId, agent, days),
}));

let mockAlerts: unknown;
vi.mock("@/lib/server/entities/alerts", () => ({ fetchAlerts: async () => mockAlerts }));

let mockChanges: unknown;
const fetchChanges = vi.fn<
  (projectId: string, days: number | undefined, agents: readonly string[], environments: readonly string[]) => Promise<unknown>
>(async () => mockChanges);
vi.mock("@/lib/server/entities/changes", () => ({
  fetchChanges: (
    projectId: string,
    days: number | undefined,
    agents: readonly string[],
    environments: readonly string[],
  ) => fetchChanges(projectId, days, agents, environments),
}));

let mockSessions: unknown;
const fetchSessions = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(async () => mockSessions);
vi.mock("@/lib/server/entities/sessions", () => ({
  fetchSessions: (projectId: string, query: unknown) => fetchSessions(projectId, query),
}));

import AgentPage, { generateMetadata } from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });
const PROJECT = "prj_1";
const LAST = "0191a3b47c2e7d3f8e4a5b6c7d8e9f01";
const FAILED = "0191a3b47c2e7d3f8e4a5b6c7d8e9f02";
const SESSION_ID = "9f86d081884c7d659a2feaa0c55ad015";

const page = (searchParams: Record<string, string | string[]>) =>
  AgentPage({ params: PARAMS, searchParams: Promise.resolve(searchParams) } as never);

const agent = (patch: Record<string, unknown> = {}) => ({
  agent: "support-bot",
  traces: 40,
  failed_traces: 4,
  faults: { timeout: 3, rate_limited: 12 },
  spend: 12.5,
  last_trace_id: LAST,
  last_started_at: "2026-11-05T09:00:00Z",
  last_ended: true,
  last_status: "ok",
  last_spend: 0.25,
  usual_spend: 0.125,
  models: ["model-a", "model-b"],
  prompt_versions: [],
  ...patch,
});

const agentsOk = (agents: unknown[], days = 30) => ({ kind: "ok", value: { days, agents, changes: [] } });

const change = (value: string, minute: number) => ({
  agent: "support-bot",
  environment: "production",
  kind: "release",
  value,
  changed_at: `2026-11-05T08:${String(minute).padStart(2, "0")}:00Z`,
  trace_id: LAST,
  capped: false,
});

const session = {
  session_id: SESSION_ID,
  user_id: "",
  agents: ["support-bot"],
  traces: 3,
  failed_traces: 0,
  input_tokens: 1200,
  output_tokens: 300,
  spend: 0.875,
  duration_ms: 3000,
  first_seen: "2026-11-05T03:00:00Z",
  last_seen: "2026-11-05T03:20:01Z",
  last_ended: true,
  last_status: "ok",
  tags: [],
  scores: [],
};

const card = (title: string) => within(screen.getByRole("region", { name: title }));

describe("an agent's page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    mockOffset = undefined;
    mockProject = { id: PROJECT, slug: "web", name: "Web", role: "member" };
    mockAgents = agentsOk([agent()]);
    mockAlerts = { kind: "ok", alerts: [], incidents: [], mutes: [] };
    mockHistogram = {
      kind: "ok",
      histogram: { days: 30, bucket_seconds: 86_400, utc_offset: 0, bars: [{ at: "2026-11-05T00:00:00Z" }] },
    };
    mockChanges = { kind: "ok", value: { days: 30, more: false, changes: [] } };
    mockFailed = { kind: "ok", list: { traces: [], total: 0 } };
    mockSessions = { kind: "ok", list: { sessions: [], total: 0 } };
  });

  it("is the Agents page's where it names no agent, or one no agent could have", async () => {
    await expect(page({})).rejects.toThrow("/acme/web");
    await expect(page({ agent: "" })).rejects.toThrow(Redirected);
    await expect(page({ agent: "a".repeat(129) })).rejects.toThrow(Redirected);
    await expect(page({ agent: ["a", "b"] })).rejects.toThrow(Redirected);
    expect(fetchAgents).not.toHaveBeenCalled();
  });

  it("titles the tab and the page by the agent, as it is named", async () => {
    expect(await generateMetadata({ searchParams: Promise.resolve({ agent: " padded " }) } as never)).toEqual({
      title: " padded ",
    });
    expect(await generateMetadata({ searchParams: Promise.resolve({}) } as never)).toEqual({ title: "Agent" });
    mockAgents = agentsOk([agent({ agent: " padded " })]);
    render(await page({ agent: " padded ", days: "30" }));
    expect(fetchAgents).toHaveBeenCalledWith(PROJECT, " padded ", 30);
    const header = screen.getByTestId("page-header");
    expect(within(header).getByRole("link", { name: "Agents" })).toHaveAttribute("href", "/acme/web");
    // A visit is filed under the address that names the agent, which the path alone
    // does not, and once whatever window it was read over.
    expect(header).toHaveAttribute("data-visit", "/acme/web/agents?agent=+padded+");
  });

  it("reads every part over the window the URL asks for, on the reader's clock", async () => {
    mockOffset = "330";
    render(await page({ agent: "support-bot", days: "30" }));
    const its = { agent: ["support-bot"], days: 30 };
    expect(fetchAgents).toHaveBeenCalledWith(PROJECT, "support-bot", 30);
    expect(fetchTraceHistogram).toHaveBeenCalledWith(PROJECT, its, 330);
    expect(fetchChanges).toHaveBeenCalledWith(PROJECT, 30, ["support-bot"], []);
    expect(fetchTraces).toHaveBeenCalledWith(PROJECT, { ...its, status: ["error"], limit: 5 });
    expect(fetchSessions).toHaveBeenCalledWith(PROJECT, { ...its, limit: 5 });
    expect(within(screen.getByTestId("page-header")).getByRole("link", { name: "Its traces" })).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=support-bot&days=30",
    );
  });

  it("opens on a week where the URL asks for none, and its links ask for none either", async () => {
    mockAgents = agentsOk([agent()], 7);
    render(await page({ agent: "support-bot" }));
    expect(fetchAgents).toHaveBeenCalledWith(PROJECT, "support-bot", 7);
    expect(fetchSessions).toHaveBeenCalledWith(PROJECT, { agent: ["support-bot"], days: 7, limit: 5 });
    expect(card("All traces").getByRole("link", { name: "Its traces" })).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=support-bot",
    );
  });

  it("says how it stands: its last trace, its traces and those that failed, what they cost, and who watches", async () => {
    mockAlerts = {
      kind: "ok",
      alerts: [
        { id: "every", filters: {}, paused: null },
        { id: "its", filters: { agent: ["support-bot"] }, paused: null },
      ],
      incidents: [{ id: "i1", rule_id: "its", status: "open" }],
      mutes: [],
    };
    render(await page({ agent: "support-bot", days: "30" }));
    const last = card("Last trace");
    expect(last.getByText("OK")).toBeInTheDocument();
    expect(last.getByText("It cost $0.25.")).toBeInTheDocument();
    // A trace the page names opens over the page, never in the trace list's place.
    expect(last.getByRole("link", { name: "Open it" })).toHaveAttribute(
      "href",
      `/acme/web/agents?agent=support-bot&days=30&peek=${LAST}`,
    );
    const all = card("All traces");
    expect(all.getByText("40")).toBeInTheDocument();
    expect(all.getByText("4 ended in error, 10%.")).toHaveClass("text-destructive");
    const spend = card("Spend");
    expect(spend.getByText("$12.50")).toBeInTheDocument();
    expect(spend.getByText("A trace usually costs $0.125.")).toBeInTheDocument();
    const watched = card("Watched by");
    expect(watched.getByText("2 alerts")).toBeInTheDocument();
    expect(watched.getByText("1 incident is open.")).toHaveClass("text-destructive");
    expect(watched.getByRole("link", { name: "Alerts" })).toHaveAttribute("href", "/acme/web/alerts");
  });

  it("says a running trace is running, a quiet agent failed nothing, and unread alerts could not be read", async () => {
    mockAgents = agentsOk([
      agent({ last_ended: false, failed_traces: 0, faults: {}, usual_spend: null }),
    ]);
    mockAlerts = { kind: "unavailable" };
    render(await page({ agent: "support-bot", days: "30" }));
    expect(card("Last trace").getByText("Still running.")).toBeInTheDocument();
    expect(card("Last trace").getByText("Running")).toBeInTheDocument();
    expect(card("All traces").getByText("None ended in error.")).toBeInTheDocument();
    expect(card("Spend").getByText("No trace has ended yet.")).toBeInTheDocument();
    expect(card("Watched by").getByText("Its alerts could not be read.")).toBeInTheDocument();
    expect(card("Failures").getByText("No trace failed, and no call of one faulted.")).toBeInTheDocument();
    expect(card("Failures").queryByRole("link")).toBeNull();
  });

  it("names what failed: its traces under each fault, in the contract's order, and the newest that ended in error", async () => {
    mockFailed = {
      kind: "ok",
      list: {
        traces: [
          { trace_id: FAILED, error_type: "TimeoutError", started_at: "2026-11-05T08:00:00Z" },
          { trace_id: LAST, error_type: "", started_at: "2026-11-05T07:00:00Z" },
        ],
        total: 4,
      },
    };
    render(await page({ agent: "support-bot", days: "30" }));
    const failures = card("Failures");
    const faults = within(failures.getByRole("list", { name: "Traces by fault" }));
    expect(faults.getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "Rate limited12 traces",
      "Timeout3 traces",
    ]);
    expect(faults.getByRole("link", { name: "Timeout" })).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=support-bot&days=30&fault=timeout",
    );
    const newest = within(failures.getByRole("list", { name: "Newest traces that ended in error" }));
    expect(newest.getByRole("link", { name: "TimeoutError" })).toHaveAttribute(
      "href",
      `/acme/web/agents?agent=support-bot&days=30&peek=${FAILED}`,
    );
    // The peek steps through the failures the page lists, as the member reading it.
    expect(screen.getByTestId("peek").dataset.traces?.split(" ")).toContain(FAILED);
    expect(screen.getByTestId("peek")).toHaveAttribute("data-me", "user_1");
    // A trace that ended in error with no class named still opens.
    expect(newest.getByRole("link", { name: "Error" })).toBeInTheDocument();
    expect(failures.getByRole("link", { name: "Its traces that ended in error" })).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=support-bot&days=30&status=error",
    );
  });

  it("lists its newest changes, each opening the trace that first carried it, and marks them all on its charts", async () => {
    const changes = [7, 6, 5, 4, 3, 2, 1].map((n) => change(`1.4.${n}`, n));
    mockChanges = { kind: "ok", value: { days: 30, more: false, changes } };
    render(await page({ agent: "support-bot", days: "30" }));
    const listed = card("Changes");
    expect(listed.getAllByRole("listitem")).toHaveLength(5);
    expect(listed.getByRole("link", { name: "release 1.4.7 in production" })).toHaveAttribute(
      "href",
      `/acme/web/agents?agent=support-bot&days=30&peek=${LAST}`,
    );
    expect(listed.getByText("And 2 before them, marked on the charts below.")).toBeInTheDocument();
    const charts = screen.getByTestId("charts");
    expect(JSON.parse(charts.dataset.marks ?? "null")).toEqual([
      { at: "2026-11-05T00:00:00Z", changes: changes.map((c) => `support-bot: release ${c.value} in production`) },
    ]);
    // The tokens chart picks among the agent's own models.
    expect(JSON.parse(charts.dataset.models ?? "null")).toEqual(["model-a", "model-b"]);
  });

  it("says the window held changes older than those read, which nothing marks", async () => {
    const changes = [6, 5, 4, 3, 2, 1].map((n) => change(`1.4.${n}`, n));
    mockChanges = { kind: "ok", value: { days: 30, more: true, changes } };
    render(await page({ agent: "support-bot", days: "30" }));
    expect(
      card("Changes").getByText("And 1 before them, marked on the charts below; the window's older ones are not."),
    ).toBeInTheDocument();
  });

  it("sets its versions side by side over the days the read answered, held to what is kept", async () => {
    mockAgents = agentsOk([agent()], 14);
    render(await page({ agent: "support-bot", days: "30" }));
    const versions = screen.getByTestId("versions");
    expect(versions.dataset).toMatchObject({ project: PROJECT, agent: "support-bot", days: "14" });
  });

  it("lists its sessions, each opening its own page, and leads to the rest where it names only some", async () => {
    mockSessions = { kind: "ok", list: { sessions: [session], total: 12 } };
    const { unmount } = render(await page({ agent: "support-bot", days: "30" }));
    expect(screen.getByRole("link", { name: "9f86d081" })).toHaveAttribute(
      "href",
      `/acme/web/sessions/${SESSION_ID}?days=30`,
    );
    // A heading sorts the sessions list, narrowed to this agent.
    expect(within(screen.getByRole("table")).getByRole("link", { name: /^Spend/ })).toHaveAttribute(
      "href",
      "/acme/web/sessions?agent=support-bot&days=30&sort=spend&dir=desc",
    );
    expect(screen.getByRole("link", { name: "All 12 of its sessions" })).toHaveAttribute(
      "href",
      "/acme/web/sessions?agent=support-bot&days=30",
    );
    unmount();

    mockSessions = { kind: "ok", list: { sessions: [], total: 0 } };
    render(await page({ agent: "support-bot", days: "30" }));
    expect(screen.getByText("None of its traces in the window named a session.")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("stands without a part that could not be read, and says which", async () => {
    mockHistogram = { kind: "unavailable" };
    mockChanges = { kind: "unavailable" };
    mockFailed = { kind: "unavailable" };
    mockSessions = { kind: "unavailable" };
    render(await page({ agent: "support-bot", days: "30" }));
    expect(screen.getByText("The charts could not be read.")).toBeInTheDocument();
    expect(card("Changes").getByText("Its changes could not be read.")).toBeInTheDocument();
    expect(card("Failures").getByText("Its failed traces could not be read.")).toBeInTheDocument();
    expect(screen.getByText("Its sessions could not be read.")).toBeInTheDocument();
    expect(card("All traces").getByText("40")).toBeInTheDocument();
  });

  it("says an agent with no trace in the window has none, and leaves the window to widen", async () => {
    mockAgents = agentsOk([], 30);
    render(await page({ agent: "never-ran", days: "30" }));
    expect(screen.getByText("No trace of this agent in the last 30 days.")).toBeInTheDocument();
    const header = screen.getByTestId("page-header");
    expect(within(header).getByRole("combobox", { name: "Window" })).toHaveTextContent("Last 30 days");
    // A name no trace carries is no page to come back to.
    expect(header).toHaveAttribute("data-visit", "none");
    expect(screen.queryByTestId("charts")).toBeNull();
  });

  it("says so on an outage, and where the read is refused, as the Agents page does", async () => {
    for (const kind of ["unavailable", "forbidden"]) {
      mockAgents = { kind };
      const { unmount } = render(await page({ agent: "support-bot" }));
      expect(screen.getByTestId("outage"), kind).toBeInTheDocument();
      unmount();
    }
    mockAgents = agentsOk([agent()]);
    mockProject = null;
    render(await page({ agent: "support-bot" }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
