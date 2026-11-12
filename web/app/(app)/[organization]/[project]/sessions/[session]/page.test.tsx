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
  SessionCharts: ({ traces }: { traces: readonly { trace_id: string }[] }) => (
    <div data-testid="charts" data-traces={traces.map((trace) => trace.trace_id).join(" ")} />
  ),
}));

// The peek reads a trace once opened, and is proved by its own suite.
vi.mock("@/components/traces/peek", () => ({
  usePeekedTrace: () => null,
  TracePeek: ({ traces }: { traces: readonly string[] }) => <div data-testid="peek" data-traces={traces.join(" ")} />,
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/acme/web/sessions/9f86d081884c7d659a2feaa0c55ad015",
  useSearchParams: () => new URLSearchParams(),
}));

let mockContext: unknown;
let mockProject: unknown;
let mockTraces: unknown;
const fetchTraces = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(async () => mockTraces);
vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: async () => mockProject,
  fetchTraces: (projectId: string, query: unknown) => fetchTraces(projectId, query),
}));

let mockSessions: unknown;
const fetchSessions = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(async () => mockSessions);
vi.mock("@/lib/server/entities/sessions", () => ({
  fetchSessions: (projectId: string, query: unknown) => fetchSessions(projectId, query),
}));
vi.mock("@/lib/server/entities/content-mode", () => ({ fetchContentMode: async () => null }));

import SessionPage, { generateMetadata } from "./page";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const ID = "9f86d081884c7d659a2feaa0c55ad015";
const USER = "2c26b46b68ffc68ff99b453c1d304134";
const FIRST = "0191a3b47c2e7d3f8e4a5b6c7d8e9f01";
const LAST = "0191a3b47c2e7d3f8e4a5b6c7d8e9f02";

const page = (id: string, searchParams: Record<string, string> = { days: "30" }) =>
  SessionPage({
    params: Promise.resolve({ organization: "acme", project: "web", session: id }),
    searchParams: Promise.resolve(searchParams),
  } as never);

const SESSION = {
  session_id: ID,
  user_id: USER,
  agents: ["support-bot", "triage"],
  traces: 3,
  failed_traces: 1,
  input_tokens: 1200,
  output_tokens: 300,
  spend: 0.875,
  duration_ms: 3000,
  first_seen: "2026-10-11T03:00:00Z",
  last_seen: "2026-10-11T03:20:01Z",
  last_ended: true,
  last_status: "error",
  tags: ["repo:telmoni"],
  scores: [{ name: "resolved", data_type: "boolean", mean: 1, count: 1, categories: [] }],
};

const trace = (trace_id: string, started_at: string, patch: Record<string, unknown> = {}) => ({
  trace_id,
  agent: "support-bot",
  user_id: USER,
  session_id: ID,
  ended: true,
  status: "ok",
  error_type: "",
  started_at,
  ended_at: started_at,
  duration_ms: 1000,
  time_to_first_token_ms: null,
  spans: 2,
  steps: 0,
  model_calls: 1,
  tool_calls: 0,
  retrievals: 0,
  errors: 0,
  input_tokens: 400,
  output_tokens: 100,
  cache_read_input_tokens: 0,
  cache_write_input_tokens: 0,
  reasoning_output_tokens: 0,
  spend: 0.25,
  models: ["model-a"],
  tools: [],
  environments: [],
  releases: [],
  tags: [],
  scores: [],
  ...patch,
});

const traces = (over: Record<string, unknown> = {}) => ({
  kind: "ok",
  list: {
    traces: [
      trace(FIRST, "2026-10-11T03:00:00Z"),
      trace(LAST, "2026-10-11T03:20:00Z", { status: "error", error_type: "TimeoutError" }),
    ],
    total: 2,
    ...over,
  },
});

const card = (title: string) => within(screen.getByRole("region", { name: title }));

describe("a session's page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    mockContext = { person: { userId: "user_1", email: "ada@example.test" }, organizations: [] };
    mockProject = { id: PROJECT, slug: "web", name: "Web", role: "member" };
    mockSessions = { kind: "ok", list: { sessions: [SESSION], days: 30, total: 1 } };
    mockTraces = traces();
  });

  it("titles the tab and the page by the session, and reads it and its traces in order over the window asked", async () => {
    expect(await generateMetadata({ params: Promise.resolve({ session: ID }) } as never)).toEqual({
      title: "Session 9f86d081",
    });
    render(await page(ID));
    expect(fetchSessions).toHaveBeenCalledExactlyOnceWith(PROJECT, { session: [ID], days: 30, limit: 1 });
    expect(fetchTraces).toHaveBeenCalledExactlyOnceWith(PROJECT, {
      session: [ID],
      sort: "started",
      dir: "asc",
      days: 30,
      limit: 500,
    });
    const header = within(screen.getByTestId("page-header"));
    expect(header.getByRole("heading")).toHaveTextContent("Session 9f86d081");
    expect(header.getByRole("link", { name: "Sessions" })).toHaveAttribute("href", "/acme/web/sessions?days=30");
    expect(header.getByRole("combobox", { name: "Window" })).toHaveTextContent("Last 30 days");
  });

  it("says how it stands and what it took, its last trace opening over the page", async () => {
    render(await page(ID));
    const last = card("Last trace");
    expect(last.getByText("It ended in error.")).toBeInTheDocument();
    expect(last.getByRole("link", { name: "Open it" })).toHaveAttribute(
      "href",
      `/acme/web/sessions/${ID}?days=30&peek=${LAST}`,
    );
    const all = card("All traces");
    expect(all.getByText("3")).toBeInTheDocument();
    expect(all.getByText(/^1 ended in error, 33/)).toHaveClass("text-destructive");
    const duration = card("Ran for");
    expect(duration.getByText("3 s")).toBeInTheDocument();
    expect(duration.getByText(/over 20 min 1 s from the first to the last/)).toBeInTheDocument();
    const spend = card("Spend");
    expect(spend.getByText("$0.875")).toBeInTheDocument();
    expect(spend.getByText("1,200 tokens in, 300 out.")).toBeInTheDocument();
  });

  it("names its user, its agents, its tags and its own scores, each opening its own page or list", async () => {
    render(await page(ID));
    expect(screen.getByRole("link", { name: "2c26b46b" })).toHaveAttribute(
      "href",
      `/acme/web/users/${USER}?days=30`,
    );
    expect(screen.getByRole("link", { name: "triage" })).toHaveAttribute(
      "href",
      "/acme/web/agents?agent=triage&days=30",
    );
    expect(screen.getByRole("link", { name: "repo:telmoni" })).toHaveAttribute(
      "href",
      "/acme/web/sessions?tag=repo%3Atelmoni&days=30",
    );
    expect(screen.getByTitle("resolved: 1 score")).toHaveTextContent("resolved yes");
  });

  it("charts its traces and lists them in the order they started, each opening beside them", async () => {
    render(await page(ID));
    expect(screen.getByTestId("charts")).toHaveAttribute("data-traces", `${FIRST} ${LAST}`);
    expect(screen.getByTestId("peek")).toHaveAttribute("data-traces", `${FIRST} ${LAST}`);
    const table = within(screen.getByRole("table"));
    const ids = table.getAllByRole("link", { name: /^0191a3b4$/ });
    expect(ids.map((link) => link.getAttribute("href"))).toEqual([
      `/acme/web/traces/${FIRST}`,
      `/acme/web/traces/${LAST}`,
    ]);
    expect(ids[1]?.closest("td")).toHaveTextContent("TimeoutError");
    // A heading sorts the trace list, narrowed to this session.
    expect(table.getByRole("link", { name: /^Duration/ })).toHaveAttribute(
      "href",
      `/acme/web/traces?session=${ID}&sort=duration&dir=desc&days=30`,
    );
    expect(screen.queryByRole("link", { name: /of its traces/ })).toBeNull();
  });

  it("says so where it holds more traces than one read, and leads to them all", async () => {
    mockTraces = traces({ total: 600 });
    render(await page(ID));
    expect(screen.getByText("The first 2 of its 600 traces are drawn.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "All 600 of its traces" })).toHaveAttribute(
      "href",
      `/acme/web/traces?session=${ID}&sort=started&dir=asc&days=30`,
    );
    // Its last trace is not among those read, so no card claims to open it.
    expect(card("Last trace").queryByRole("link", { name: "Open it" })).toBeNull();
  });

  it("keeps the page where its traces cannot be read", async () => {
    mockTraces = { kind: "unavailable" };
    render(await page(ID));
    expect(card("All traces").getByText("3")).toBeInTheDocument();
    expect(screen.getAllByText("Its traces could not be read.")).toHaveLength(2);
    expect(screen.queryByTestId("charts")).toBeNull();
    expect(screen.queryByTestId("peek")).toBeNull();
  });

  it("says so of a session the window holds no trace of, and of an id past what one is stored as", async () => {
    mockSessions = { kind: "ok", list: { sessions: [], days: 7, total: 0 } };
    const { unmount } = render(await page(ID, {}));
    expect(fetchSessions).toHaveBeenCalledWith(PROJECT, { session: [ID], days: 7, limit: 1 });
    expect(screen.getByText("No trace of this session in the last 7 days.")).toBeInTheDocument();
    expect(screen.getByTestId("page-header")).toHaveAttribute("data-visit", "none");
    expect(screen.getByRole("combobox", { name: "Window" })).toBeInTheDocument();
    unmount();

    vi.clearAllMocks();
    render(await page("s".repeat(201), {}));
    expect(screen.getByText("No trace of this session in the last 7 days.")).toBeInTheDocument();
    // The way back, above the title and in the notice, both to the list.
    expect(screen.getAllByRole("link", { name: "Sessions" }).map((link) => link.getAttribute("href"))).toEqual([
      "/acme/web/sessions",
      "/acme/web/sessions",
    ]);
    expect(fetchSessions).not.toHaveBeenCalled();
    expect(fetchTraces).not.toHaveBeenCalled();
  });

  it("sends a refused role back to the project's overview, and says so on an outage", async () => {
    mockSessions = { kind: "forbidden" };
    render(await page(ID));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute("href", "/acme/web");

    mockSessions = { kind: "unavailable" };
    render(await page(ID));
    expect(screen.getByTestId("outage")).toBeInTheDocument();

    mockProject = null;
    render(await page(ID));
    expect(screen.getAllByTestId("outage")).toHaveLength(2);
  });
});
