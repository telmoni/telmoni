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
  usePathname: () => "/acme/web/users",
  useSearchParams: () => new URLSearchParams(),
}));

import { TRACE_PREFS_KEY } from "@/lib/trace-prefs";
import { Role } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
const fetchTraces = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>();
vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: async () => mockProject,
  fetchTraces: (projectId: string, query: unknown) => fetchTraces(projectId, query),
}));

let mockUsers: unknown;
let mockFacets: unknown;
const fetchUsers = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(async () => mockUsers);
const fetchUserFacets = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(async () => mockFacets);
vi.mock("@/lib/server/entities/users", () => ({
  fetchUsers: (projectId: string, query: unknown) => fetchUsers(projectId, query),
  fetchUserFacets: (projectId: string, query: unknown) => fetchUserFacets(projectId, query),
}));

import UsersPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });
const PROJECT = "project_7bQx2mNv9BcK4dLp";
const USER = "2c26b46b68ffc68ff99b453c1d304134";

const BUSY = {
  user_id: USER,
  sessions: 2,
  agents: ["support-bot", "triage"],
  traces: 4,
  failed_traces: 1,
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
const ONCE = {
  ...BUSY,
  user_id: "486ea46224d1bb4fb680f34f7c9ad96a",
  sessions: 0,
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
};

function list(over: Record<string, unknown> = {}) {
  return {
    kind: "ok",
    list: {
      users: [BUSY, ONCE],
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
  mockUsers = list();
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

describe("UsersPage", () => {
  it("lists a page of end users, each summed, their id opening their own page", async () => {
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Users");
    expect(screen.getByText("Total 2")).toBeInTheDocument();
    // A page holds fifty rows unless the URL names another count.
    expect(fetchUsers).toHaveBeenCalledWith(PROJECT, { limit: 50 });
    expect(fetchUserFacets).toHaveBeenCalledWith(PROJECT, {});

    const table = within(screen.getByRole("table"));
    const id = table.getByRole("link", { name: "2c26b46b" });
    expect(id).toHaveAttribute("href", `/acme/web/users/${USER}`);
    expect(id.closest("td")).toHaveAttribute("title", USER);
    const row = within(id.closest("tr")!);
    // Each of their agents narrows the users list, never the traces'.
    expect(row.getByRole("link", { name: "triage" })).toHaveAttribute("href", "/acme/web/users?agent=triage");
    // Their last trace's agent span has not arrived: running, whatever its status.
    expect(row.getByText("Running")).toBeInTheDocument();
    // How many sessions they had opens those sessions.
    expect(row.getByRole("link", { name: "2" })).toHaveAttribute("href", `/acme/web/sessions?user=${USER}`);
    expect(row.getByText("4")).toBeInTheDocument();
    expect(row.getByText("1")).toHaveClass("text-destructive");
    expect(row.getByText("1,500")).toHaveAttribute("title", "1,200 in, 300 out");
    expect(row.getByText("$0.875")).toBeInTheDocument();
    // One whose traces named no session has none to open.
    const once = within(table.getByRole("link", { name: "486ea462" }).closest("tr")!);
    expect(once.getByText("Error")).toBeInTheDocument();
    expect(once.queryByRole("link", { name: "0" })).toBeNull();
    expect(table.getAllByRole("columnheader").map((heading) => heading.textContent)).toEqual([
      "User",
      "Agent",
      "Last trace",
      "Sessions",
      "Traces",
      "Failed",
      "Tokens",
      "Spend",
    ]);
    expect(screen.getByRole("button", { name: /Columns/ })).toHaveTextContent("8/11");
    const header = within(screen.getByTestId("page-header"));
    expect(header.getByRole("combobox", { name: "Window" })).toHaveTextContent("Last 7 days");
    expect(header.getByRole("button", { name: "Download" })).toBeInTheDocument();
  });

  it("opens a user's page from anywhere on their row, over the list's window, and leaves a link to its own way", async () => {
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({ days: "30" }) }));
    const row = screen.getByRole("link", { name: "2c26b46b" }).closest("tr")!;
    fireEvent.click(within(row).getByText("$0.875"));
    expect(push).toHaveBeenCalledExactlyOnceWith(`/acme/web/users/${USER}?days=30`);
    fireEvent.click(within(row).getByRole("link", { name: "2" }));
    expect(push).toHaveBeenCalledTimes(1);
    expect(within(row).getByRole("link", { name: "2" })).toHaveAttribute(
      "href",
      `/acme/web/sessions?user=${USER}&days=30`,
    );
  });

  it("reads each filter's values, the sort, the window and the page, and hands them to the server", async () => {
    mockUsers = list({ sort: "sessions", dir: "asc", days: 30, limit: 25, page: 2, total: 60, more: true });
    render(
      await UsersPage({
        params: PARAMS,
        searchParams: Promise.resolve({
          agent: ["support-bot", "triage"],
          status: "error",
          sort: "sessions",
          dir: "asc",
          days: "30",
          limit: "25",
          page: "2",
        }),
      }),
    );
    const query = {
      agent: ["support-bot", "triage"],
      status: ["error"],
      sort: "sessions",
      dir: "asc",
      days: 30,
      page: 2,
    };
    expect(fetchUsers).toHaveBeenCalledWith(PROJECT, { ...query, limit: 25 });
    expect(fetchUserFacets).toHaveBeenCalledWith(PROJECT, { ...query, limit: 25 });
    // A heading sorts the list by it, from the first page, the filters kept.
    expect(screen.getByRole("link", { name: /^Traces/ })).toHaveAttribute(
      "href",
      "/acme/web/users?agent=support-bot&agent=triage&status=error&sort=traces&dir=desc&days=30&limit=25",
    );
    expect(screen.getByRole("columnheader", { name: /Sessions/ })).toHaveAttribute("aria-sort", "ascending");
  });

  it("shows how long their traces ran, their first and their tags where a person asks for them", async () => {
    window.localStorage.setItem(
      TRACE_PREFS_KEY,
      JSON.stringify({ userColumns: ["tags", "duration", "first_seen", "scores"] }),
    );
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const table = within(screen.getByRole("table"));
    // In the table's order, a column of another list's let go.
    expect(table.getAllByRole("columnheader").map((heading) => heading.textContent)).toEqual([
      "User",
      "Duration",
      "First trace",
      "Tags",
    ]);
    const row = within(table.getByRole("link", { name: "2c26b46b" }).closest("tr")!);
    expect(row.getByText("4 s")).toBeInTheDocument();
    expect(row.getByRole("link", { name: "repo:telmoni" })).toHaveAttribute(
      "href",
      "/acme/web/users?tag=repo%3Atelmoni",
    );
  });

  // ⚠ The state most projects are in: traces, and none that names a user.
  it("tells a project whose traces name no user how to name one, by one read of the window's traces", async () => {
    mockUsers = list({ users: [], total: 0 });
    fetchTraces.mockResolvedValue({ kind: "ok", list: { traces: [], total: 12 } });
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({ days: "30" }) }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.getByText(/named the user it was for/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "SDKs" })).toHaveAttribute("href", "/docs/api/sdks");
    expect(screen.getByRole("combobox", { name: "Window" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "See your first user" })).toBeNull();
    // Never all the plan keeps, which every redraw would pay for.
    expect(fetchTraces).toHaveBeenCalledExactlyOnceWith(PROJECT, { days: 30, limit: 1 });
    expect(fetchUsers).toHaveBeenCalledTimes(1);
  });

  it("tells a filtered window to widen, with no read beside the list's own", async () => {
    mockUsers = list({ users: [], total: 0 });
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({ agent: "zeta", sort: "spend" }) }));
    expect(screen.getByText("No users match these filters.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "clear the filters" })).toHaveAttribute("href", "/acme/web/users?sort=spend");
    expect(fetchTraces).not.toHaveBeenCalled();
  });

  it("shows setup instead of list controls only until the project's first trace arrives", async () => {
    mockUsers = list({ users: [], total: 0 });
    fetchTraces.mockResolvedValue({ kind: "ok", list: { traces: [], total: 0 } });
    const { unmount } = render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("heading", { name: "See your first user" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Listening for traces with a user ID");
    expect(screen.getByText("user_id")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByRole("combobox", { name: "Window" })).toBeNull();
    // The window first, then all the plan keeps, each a read of one trace.
    expect(fetchTraces.mock.calls.map(([, query]) => query)).toEqual([{ limit: 1 }, { days: 360, limit: 1 }]);
    unmount();

    // A project whose traces are all older than the window is no first run.
    fetchTraces.mockReset();
    fetchTraces
      .mockResolvedValueOnce({ kind: "ok", list: { traces: [], total: 0 } })
      .mockResolvedValueOnce({ kind: "ok", list: { traces: [], total: 3 } });
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "See your first user" })).toBeNull();
  });

  it("draws the quiet window, never the setup, where the project's traces cannot be read", async () => {
    mockUsers = list({ users: [], total: 0 });
    fetchTraces.mockResolvedValue({ kind: "unavailable" });
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(fetchTraces).toHaveBeenCalledTimes(1);
  });

  it("sends a refused role back to the project's overview, and says so on an outage", async () => {
    mockUsers = { kind: "forbidden" };
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute("href", "/acme/web");

    mockUsers = { kind: "unavailable" };
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();

    mockContext = null;
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getAllByTestId("outage")).toHaveLength(2);
  });
});
