// @vitest-environment jsdom
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, toolbar }: { title: string; toolbar?: React.ReactNode }) => (
    <div data-testid="page-header">
      {title}
      {toolbar}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

vi.mock("@/app/(app)/[organization]/[project]/traces/actions", () => ({}));

const { push } = vi.hoisted(() => ({ push: vi.fn() }));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, refresh: vi.fn() }),
  usePathname: () => "/acme/web/sessions",
  useSearchParams: () => new URLSearchParams(),
}));

import { TRACE_PREFS_KEY } from "@/lib/trace-prefs";
import { Role } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: async () => mockProject,
}));

let mockSessions: unknown;
let mockFacets: unknown;
const fetchSessions = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(async () => mockSessions);
const fetchSessionFacets = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(async () => mockFacets);
vi.mock("@/lib/server/entities/sessions", () => ({
  fetchSessions: (projectId: string, query: unknown) => fetchSessions(projectId, query),
  fetchSessionFacets: (projectId: string, query: unknown) => fetchSessionFacets(projectId, query),
}));

import SessionsPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });
const PROJECT = "project_7bQx2mNv9BcK4dLp";
const SESSION_ID = "9f86d081884c7d659a2feaa0c55ad015";
const USER = "2c26b46b68ffc68ff99b453c1d304134";

const LONG = {
  session_id: SESSION_ID,
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
  last_ended: false,
  last_status: "ok",
  tags: ["repo:telmoni"],
  scores: [{ name: "resolved", data_type: "boolean", mean: 1, count: 1, categories: [] }],
};
const SHORT = {
  ...LONG,
  session_id: "fcde2b2edba56bf408601fb721fe9b5c",
  user_id: "",
  agents: ["nightly-report"],
  traces: 1,
  failed_traces: 0,
  input_tokens: 10,
  output_tokens: 5,
  spend: 0.0625,
  duration_ms: 500,
  last_ended: true,
  last_status: "error",
  tags: [],
  scores: [],
};

function list(over: Record<string, unknown> = {}) {
  return {
    kind: "ok",
    list: {
      sessions: [LONG, SHORT],
      days: 7,
      limit: 50,
      page: 1,
      total: 2,
      more: false,
      sort: "last_seen",
      dir: "desc",
      ...over,
    },
  };
}

const NONE = { values: [], more: false };

beforeAll(() => {
  // cmdk keeps the item in focus in view, which jsdom cannot scroll.
  Element.prototype.scrollIntoView = vi.fn();
});

beforeEach(() => {
  vi.clearAllMocks();
  window.localStorage.clear();
  mockContext = {
    person: { userId: "user_1", email: "ada@example.test" },
    organizations: [],
    activeOrganizationId: "org_1",
    memberships: [],
    incomingInvites: [],
    flags: {},
  };
  mockProject = { id: PROJECT, slug: "web", name: "Web", role: Role.Member };
  mockSessions = list();
  mockFacets = {
    kind: "ok",
    facets: {
      days: 7,
      agent: { values: [{ value: "support-bot", count: 1 }], more: false },
      user: NONE,
      session: NONE,
      status: NONE,
      model: NONE,
      tool: NONE,
      environment: NONE,
      release: NONE,
      tag: NONE,
      prompt: NONE,
      prompt_version: NONE,
      fault: NONE,
    },
  };
});

describe("SessionsPage", () => {
  it("lists a page of sessions, each summed, its id opening its own page", async () => {
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Sessions");
    expect(screen.getByText("Total 2")).toBeInTheDocument();
    // A page holds fifty rows unless the URL names another count.
    expect(fetchSessions).toHaveBeenCalledWith(PROJECT, { limit: 50 });
    expect(fetchSessionFacets).toHaveBeenCalledWith(PROJECT, {});

    const table = within(screen.getByRole("table"));
    const id = table.getByRole("link", { name: "9f86d081" });
    expect(id).toHaveAttribute("href", `/acme/web/sessions/${SESSION_ID}`);
    expect(id.closest("td")).toHaveAttribute("title", SESSION_ID);
    const row = within(id.closest("tr")!);
    // Each of its agents narrows the sessions list, never the traces'.
    expect(row.getByRole("link", { name: "triage" })).toHaveAttribute("href", "/acme/web/sessions?agent=triage");
    // Its last trace's agent span has not arrived: running, whatever its status.
    expect(row.getByText("Running")).toBeInTheDocument();
    expect(row.getByText("3")).toBeInTheDocument();
    expect(row.getByText("3 s")).toBeInTheDocument();
    expect(row.getByText("1,500")).toHaveAttribute("title", "1,200 in, 300 out");
    expect(row.getByText("$0.875")).toBeInTheDocument();
    expect(row.getByRole("link", { name: "repo:telmoni" })).toHaveAttribute(
      "href",
      "/acme/web/sessions?tag=repo%3Atelmoni",
    );
    // The session's own score, drawn as the trace list draws a trace's.
    const scored = row.getByTitle("resolved: 1 score");
    expect(scored).toHaveTextContent("resolved yes");
    expect(scored.closest("td")).toHaveAttribute("title", "resolved: yes");
    expect(within(table.getByRole("link", { name: "fcde2b2e" }).closest("tr")!).getByText("Error")).toBeInTheDocument();
    // An id stored as a hash is a column to ask for: the user's is not shown.
    expect(table.queryByRole("columnheader", { name: /User/ })).toBeNull();
    expect(table.queryByRole("columnheader", { name: /Failed/ })).toBeNull();
    expect(screen.getByRole("button", { name: /Columns/ })).toHaveTextContent("9/12");
    const header = within(screen.getByTestId("page-header"));
    expect(header.getByRole("combobox", { name: "Window" })).toHaveTextContent("Last 7 days");
    expect(header.getByRole("button", { name: "Download" })).toBeInTheDocument();
  });

  it("opens a session's page from anywhere on its row, over the list's window, and leaves a link to its own way", async () => {
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({ days: "30" }) }));
    const row = screen.getByRole("link", { name: "9f86d081" }).closest("tr")!;
    fireEvent.click(within(row).getByText("3 s"));
    expect(push).toHaveBeenCalledExactlyOnceWith(`/acme/web/sessions/${SESSION_ID}?days=30`);
    fireEvent.click(within(row).getByRole("link", { name: "triage" }));
    expect(push).toHaveBeenCalledTimes(1);
  });

  it("reads each filter's values, the sort, the window and the page, and hands them to the server", async () => {
    mockSessions = list({ sort: "spend", dir: "asc", days: 30, limit: 25, page: 2, total: 60, more: true });
    render(
      await SessionsPage({
        params: PARAMS,
        searchParams: Promise.resolve({
          agent: ["support-bot", "triage"],
          status: "error",
          sort: "spend",
          dir: "asc",
          days: "30",
          limit: "25",
          page: "2",
        }),
      }),
    );
    const query = { agent: ["support-bot", "triage"], status: ["error"], sort: "spend", dir: "asc", days: 30, page: 2 };
    expect(fetchSessions).toHaveBeenCalledWith(PROJECT, { ...query, limit: 25 });
    expect(fetchSessionFacets).toHaveBeenCalledWith(PROJECT, { ...query, limit: 25 });
    // A heading sorts the list by it, from the first page, the filters kept.
    expect(screen.getByRole("link", { name: /^Traces/ })).toHaveAttribute(
      "href",
      "/acme/web/sessions?agent=support-bot&agent=triage&status=error&sort=traces&dir=desc&days=30&limit=25",
    );
    expect(screen.getByRole("columnheader", { name: /Spend/ })).toHaveAttribute("aria-sort", "ascending");
  });

  it("shows a session's user, its start and its failures where a person asks for them", async () => {
    window.localStorage.setItem(
      TRACE_PREFS_KEY,
      JSON.stringify({ sessionColumns: ["failed_traces", "first_seen", "user"] }),
    );
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const table = within(screen.getByRole("table"));
    const row = within(table.getByRole("link", { name: "9f86d081" }).closest("tr")!);
    // Its user opens the user's own page, never the list narrowed to them.
    expect(row.getByRole("link", { name: "2c26b46b" })).toHaveAttribute("href", `/acme/web/users/${USER}`);
    expect(row.getByText("1")).toHaveClass("text-destructive");
    expect(table.getAllByRole("columnheader").map((heading) => heading.textContent)).toEqual([
      "Session",
      "Failed",
      "Started",
      "User",
    ]);
  });

  it("tells a quiet window how a session is named, and a filtered one to widen", async () => {
    fetchSessions.mockResolvedValueOnce(list({ sessions: [], total: 0 }));
    const { unmount } = render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.getByText(/named the session it belongs to/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "SDKs" })).toHaveAttribute("href", "/docs/api/sdks");
    unmount();

    mockSessions = list({ sessions: [], total: 0 });
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({ agent: "zeta", sort: "spend" }) }));
    expect(screen.getByText("No sessions match these filters.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "clear the filters" })).toHaveAttribute(
      "href",
      "/acme/web/sessions?sort=spend",
    );
  });

  it("shows setup instead of list controls until the first session arrives", async () => {
    mockSessions = list({ sessions: [], total: 0 });
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("heading", { name: "Send your first session" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Listening for traces with a session ID");
    expect(screen.getByText("session_id")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByRole("combobox", { name: "Window" })).toBeNull();
    expect(screen.queryByText("Nothing to show yet.")).toBeNull();
    expect(screen.queryByRole("link", { name: "Mint a key" })).toBeNull();
  });

  it("keeps the list when its facets cannot be read", async () => {
    mockFacets = { kind: "unavailable" };
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("table")).toBeInTheDocument();
    expect(screen.getByText(/The filters.*could not be read/)).toBeInTheDocument();
  });

  it("sends a refused role back to the project's overview, and says so on an outage", async () => {
    mockSessions = { kind: "forbidden" };
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute("href", "/acme/web");

    mockSessions = { kind: "unavailable" };
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();

    mockContext = null;
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getAllByTestId("outage")).toHaveLength(2);
  });
});
