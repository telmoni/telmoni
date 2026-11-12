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
  TraceCharts: ({ histogram }: { histogram: { days: number } }) => (
    <div data-testid="charts" data-days={histogram.days} />
  ),
}));

// The peek reads a trace once opened, and is proved by its own suite.
vi.mock("@/components/traces/peek", () => ({
  usePeekedTrace: () => null,
  TracePeek: ({ traces }: { traces: readonly string[] }) => <div data-testid="peek" data-traces={traces.join(" ")} />,
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/acme/web/users/2c26b46b68ffc68ff99b453c1d304134",
  useSearchParams: () => new URLSearchParams(),
}));

let mockOffset: string | undefined;
vi.mock("next/headers", () => ({
  cookies: async () => ({
    get: (name: string) =>
      name === "telmoni-utc-offset" && mockOffset !== undefined ? { value: mockOffset } : undefined,
  }),
}));

let mockContext: unknown;
let mockProject: unknown;
let mockHistogram: unknown;
let mockTraces: unknown;
const fetchTraceHistogram = vi.fn<(projectId: string, query: unknown, utcOffset: number) => Promise<unknown>>(
  async () => mockHistogram,
);
const fetchTraces = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(async () => mockTraces);
vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: async () => mockProject,
  fetchTraceHistogram: (projectId: string, query: unknown, utcOffset: number) =>
    fetchTraceHistogram(projectId, query, utcOffset),
  fetchTraces: (projectId: string, query: unknown) => fetchTraces(projectId, query),
}));

let mockUsers: unknown;
const fetchUsers = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(async () => mockUsers);
vi.mock("@/lib/server/entities/users", () => ({
  fetchUsers: (projectId: string, query: unknown) => fetchUsers(projectId, query),
}));

let mockSessions: unknown;
const fetchSessions = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(async () => mockSessions);
vi.mock("@/lib/server/entities/sessions", () => ({
  fetchSessions: (projectId: string, query: unknown) => fetchSessions(projectId, query),
}));
vi.mock("@/lib/server/entities/content-mode", () => ({ fetchContentMode: async () => null }));

import UserPage, { generateMetadata } from "./page";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const ID = "2c26b46b68ffc68ff99b453c1d304134";
const SESSION_ID = "9f86d081884c7d659a2feaa0c55ad015";
const NEWEST = "0191a3b47c2e7d3f8e4a5b6c7d8e9f02";
const OLDER = "0191a3b47c2e7d3f8e4a5b6c7d8e9f01";

const page = (id: string, searchParams: Record<string, string> = { days: "30" }) =>
  UserPage({
    params: Promise.resolve({ organization: "acme", project: "web", user: id }),
    searchParams: Promise.resolve(searchParams),
  } as never);

const SUMMED = {
  agents: ["support-bot", "triage"],
  traces: 4,
  failed_traces: 0,
  input_tokens: 1200,
  output_tokens: 300,
  spend: 0.875,
  duration_ms: 4000,
  first_seen: "2026-10-11T03:00:00Z",
  last_seen: "2026-10-11T03:20:01Z",
  last_ended: false,
  last_status: "ok",
  tags: ["repo:telmoni"],
};
const USER = { ...SUMMED, user_id: ID, sessions: 2 };
const SESSION = { ...SUMMED, session_id: SESSION_ID, user_id: ID, traces: 3, scores: [] };

const trace = (trace_id: string, started_at: string) => ({
  trace_id,
  agent: "support-bot",
  user_id: ID,
  session_id: SESSION_ID,
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
});

const card = (title: string) => within(screen.getByRole("region", { name: title }));

describe("an end user's page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    mockOffset = undefined;
    mockContext = { person: { userId: "user_1", email: "ada@example.test" }, organizations: [] };
    mockProject = { id: PROJECT, slug: "web", name: "Web", role: "member" };
    mockUsers = { kind: "ok", list: { users: [USER], days: 30, total: 1 } };
    mockHistogram = { kind: "ok", histogram: { days: 30, bucket_seconds: 86_400, utc_offset: 0, bars: [] } };
    mockSessions = { kind: "ok", list: { sessions: [SESSION], total: 1 } };
    mockTraces = {
      kind: "ok",
      list: { traces: [trace(NEWEST, "2026-10-11T03:20:00Z"), trace(OLDER, "2026-10-11T03:00:00Z")], total: 2 },
    };
  });

  it("titles the tab and the page by the user, and reads them over the window asked, on the reader's clock", async () => {
    expect(await generateMetadata({ params: Promise.resolve({ user: ID }) } as never)).toEqual({
      title: "User 2c26b46b",
    });
    mockOffset = "-360";
    render(await page(ID));
    expect(fetchUsers).toHaveBeenCalledExactlyOnceWith(PROJECT, { user: [ID], days: 30, limit: 1 });
    expect(fetchTraceHistogram).toHaveBeenCalledExactlyOnceWith(PROJECT, { user: [ID], days: 30 }, -360);
    expect(fetchSessions).toHaveBeenCalledExactlyOnceWith(PROJECT, { user: [ID], days: 30, limit: 5 });
    expect(fetchTraces).toHaveBeenCalledExactlyOnceWith(PROJECT, { user: [ID], days: 30, limit: 25 });
    const header = within(screen.getByTestId("page-header"));
    expect(header.getByRole("heading")).toHaveTextContent("User 2c26b46b");
    expect(header.getByRole("link", { name: "Users" })).toHaveAttribute("href", "/acme/web/users?days=30");
    expect(header.getByRole("combobox", { name: "Window" })).toHaveTextContent("Last 30 days");
    expect(screen.getByTestId("charts")).toHaveAttribute("data-days", "30");
  });

  it("says how they stand and what serving them took, their last trace opening over the page", async () => {
    render(await page(ID));
    const last = card("Last trace");
    expect(last.getByText("Still running.")).toBeInTheDocument();
    expect(last.getByRole("link", { name: "Open it" })).toHaveAttribute(
      "href",
      `/acme/web/users/${ID}?days=30&peek=${NEWEST}`,
    );
    const all = card("All traces");
    expect(all.getByText("4")).toBeInTheDocument();
    expect(all.getByText("None ended in error.")).toBeInTheDocument();
    const sessions = card("Sessions");
    expect(sessions.getByText("2")).toBeInTheDocument();
    expect(sessions.getByRole("link", { name: "Their sessions" })).toHaveAttribute(
      "href",
      `/acme/web/sessions?user=${ID}&days=30`,
    );
    expect(card("Spend").getByText("1,200 tokens in, 300 out.")).toBeInTheDocument();
    // Each agent opens its own page and each tag the users it is on; the
    // sessions' rows below carry the same names, narrowing their own list.
    const hrefs = (name: string) => screen.getAllByRole("link", { name }).map((link) => link.getAttribute("href"));
    expect(hrefs("triage")).toContain("/acme/web/agents?agent=triage&days=30");
    expect(hrefs("repo:telmoni")).toContain("/acme/web/users?tag=repo%3Atelmoni&days=30");
  });

  it("lists their sessions and their newest traces, a session opening its own page and a trace opening beside them", async () => {
    mockSessions = { kind: "ok", list: { sessions: [SESSION], total: 9 } };
    mockTraces = {
      kind: "ok",
      list: { traces: [trace(NEWEST, "2026-10-11T03:20:00Z"), trace(OLDER, "2026-10-11T03:00:00Z")], total: 40 },
    };
    render(await page(ID));
    expect(screen.getByRole("link", { name: "9f86d081" })).toHaveAttribute(
      "href",
      `/acme/web/sessions/${SESSION_ID}?days=30`,
    );
    expect(screen.getByRole("link", { name: "All 9 of their sessions" })).toHaveAttribute(
      "href",
      `/acme/web/sessions?user=${ID}&days=30`,
    );
    expect(screen.getAllByRole("link", { name: /^0191a3b4$/ }).map((link) => link.getAttribute("href"))).toEqual([
      `/acme/web/traces/${NEWEST}`,
      `/acme/web/traces/${OLDER}`,
    ]);
    expect(screen.getByRole("link", { name: "All 40 of their traces" })).toHaveAttribute(
      "href",
      `/acme/web/traces?user=${ID}&days=30`,
    );
    expect(screen.getByTestId("peek")).toHaveAttribute("data-traces", `${NEWEST} ${OLDER}`);
  });

  it("keeps the page where a part of it cannot be read", async () => {
    mockHistogram = { kind: "unavailable" };
    mockSessions = { kind: "unavailable" };
    mockTraces = { kind: "unavailable" };
    render(await page(ID));
    expect(card("All traces").getByText("4")).toBeInTheDocument();
    expect(screen.getByText("The charts could not be read.")).toBeInTheDocument();
    expect(screen.getByText("Their sessions could not be read.")).toBeInTheDocument();
    expect(screen.getByText("Their traces could not be read.")).toBeInTheDocument();
    expect(card("Last trace").queryByRole("link", { name: "Open it" })).toBeNull();
    expect(screen.queryByTestId("peek")).toBeNull();
  });

  it("says so of a user the window holds no trace for, and of an id past what one is stored as", async () => {
    mockUsers = { kind: "ok", list: { users: [], days: 7, total: 0 } };
    const { unmount } = render(await page(ID, {}));
    expect(fetchUsers).toHaveBeenCalledWith(PROJECT, { user: [ID], days: 7, limit: 1 });
    expect(screen.getByText("No trace for this user in the last 7 days.")).toBeInTheDocument();
    expect(screen.getByTestId("page-header")).toHaveAttribute("data-visit", "none");
    unmount();

    vi.clearAllMocks();
    render(await page("u".repeat(129), {}));
    expect(screen.getByText("No trace for this user in the last 7 days.")).toBeInTheDocument();
    expect(fetchUsers).not.toHaveBeenCalled();
    expect(fetchTraces).not.toHaveBeenCalled();
  });

  it("sends a refused role back to the project's overview, and says so on an outage", async () => {
    mockUsers = { kind: "forbidden" };
    render(await page(ID));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute("href", "/acme/web");

    mockUsers = { kind: "unavailable" };
    render(await page(ID));
    expect(screen.getByTestId("outage")).toBeInTheDocument();

    mockProject = null;
    render(await page(ID));
    expect(screen.getAllByTestId("outage")).toHaveLength(2);
  });
});
