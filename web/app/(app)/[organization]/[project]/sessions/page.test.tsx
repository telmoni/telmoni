// @vitest-environment jsdom
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({
    title,
    info,
    action,
  }: {
    title: string;
    info?: string;
    action?: React.ReactNode;
  }) => (
    <div data-testid="page-header" data-info={info}>
      {title}
      {action}
    </div>
  ),
}));

// The window picker reads and writes the URL through the router; the
// session opened beside the list is the URL's `?peek=`.
let mockSearch = "";
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/acme/web/sessions",
  useSearchParams: () => new URLSearchParams(mockSearch),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

// The panel is drawn in the console's content area.
vi.mock("@/components/console-shell", () => ({
  useConsoleShell: () => ({ element: document.body, cover: () => () => {} }),
}));

vi.mock("@/app/(app)/[organization]/[project]/traces/actions", () => ({
  saveTraceViewAction: vi.fn(),
  updateTraceViewAction: vi.fn(),
  deleteTraceViewAction: vi.fn(),
  setSessionLayoutAction: vi.fn(),
}));

import { TRACE_PREFS_KEY } from "@/lib/trace-prefs";
import { Role } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
let mockSessions: unknown;
let mockFacets: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchSessions = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(
  async () => mockSessions,
);
const fetchSessionFacets = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(
  async () => mockFacets,
);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchSessions: (projectId: string, query: unknown) => fetchSessions(projectId, query),
  fetchSessionFacets: (projectId: string, query: unknown) => fetchSessionFacets(projectId, query),
  fetchSessionLayout: async () => ({ view_id: null }),
  fetchTraceLayouts: async () => ({ kind: "ok", layouts: [] }),
}));

import SessionsPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });
const PROJECT = "project_7bQx2mNv9BcK4dLp";
const HASHED = "9f86d081884c7d659a2feaa0c55ad015";

const SESSION_A = {
  session_id: "conv-1",
  user_id: HASHED,
  traces: 3,
  errors: 1,
  steps: 9,
  input_tokens: 1200,
  output_tokens: 300,
  first_seen: "2026-10-11T03:00:00Z",
  last_seen: "2026-10-11T03:04:02Z",
};
// An id of the application's choosing, with a slash and a space, and no user.
const SESSION_B = {
  session_id: "chat/42 b",
  user_id: "",
  traces: 1,
  errors: 0,
  steps: 2,
  input_tokens: 10,
  output_tokens: 5,
  first_seen: "2026-10-10T03:00:00Z",
  last_seen: "2026-10-10T03:00:01Z",
};

function list(over: Record<string, unknown> = {}) {
  return {
    kind: "ok",
    list: {
      sessions: [SESSION_A, SESSION_B],
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
  mockSearch = "";
  window.history.replaceState(null, "", "/acme/web/sessions");
  mockContext = {
    person: { userId: "user_1", email: "ada@example.test", analyticsOptIn: false },
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
      agent: { values: [{ value: "support-bot", count: 2 }], more: false },
      user: { values: [{ value: HASHED, count: 1 }], more: false },
      session: NONE,
      status: {
        values: [
          { value: "ok", count: 2 },
          { value: "error", count: 1 },
        ],
        more: false,
      },
      model: NONE,
      tool: NONE,
      environment: NONE,
      release: NONE,
      tag: NONE,
      prompt: NONE,
    },
  };
});

describe("SessionsPage", () => {
  it("lists a page of sessions with their users and sums, the session opening beside the list — its page from a new tab — and the user their traces", async () => {
    render(await SessionsPage({ params: PARAMS }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Sessions");
    expect(screen.getByTestId("page-header").dataset.info).toMatch(/^Every session this project's agents/);
    // A page holds fifty rows unless the URL names another count.
    expect(fetchSessions).toHaveBeenCalledWith(PROJECT, { limit: 50 });
    expect(fetchSessionFacets).toHaveBeenCalledWith(PROJECT, {});
    expect(screen.getByText("Total 2")).toBeInTheDocument();
    expect(screen.getByText("Page 1 of 1")).toBeInTheDocument();

    expect(screen.getByRole("link", { name: "conv-1" })).toHaveAttribute(
      "href",
      "/acme/web/sessions/conv-1",
    );
    expect(screen.getByRole("link", { name: "chat/42 b" })).toHaveAttribute(
      "href",
      "/acme/web/sessions/chat%2F42%20b",
    );
    const table = within(screen.getByRole("table"));
    expect(table.getByRole("link", { name: HASHED })).toHaveAttribute(
      "href",
      `/acme/web/traces?user=${HASHED}`,
    );
    // A plain click opens the session beside the list, in the URL; a click
    // for a new tab keeps the link's page.
    fireEvent.click(screen.getByRole("link", { name: "chat/42 b" }));
    expect(new URL(window.location.href).searchParams.get("peek")).toBe("chat/42 b");
    expect(screen.getByRole("link", { name: "conv-1" })).toHaveAttribute("data-peek-session", "conv-1");
    // A session whose runs named no user shows a dash, not a link.
    expect(table.getByText("—")).toBeInTheDocument();
    expect(table.getByText("1,500")).toHaveAttribute("title", "1,200 in, 300 out");

    // Last heard from first by default; a click turns it round, and another column starts its own way.
    expect(table.getByRole("columnheader", { name: /Last seen/ })).toHaveAttribute(
      "aria-sort",
      "descending",
    );
    expect(table.getByRole("link", { name: /Last seen/ })).toHaveAttribute(
      "href",
      "/acme/web/sessions?sort=last_seen&dir=asc",
    );
    expect(table.getByRole("link", { name: "Session" })).toHaveAttribute(
      "href",
      "/acme/web/sessions?sort=session&dir=asc",
    );
    expect(table.getByRole("link", { name: "Errors" })).toHaveAttribute(
      "href",
      "/acme/web/sessions?sort=errors&dir=desc",
    );
    // The tokens' halves are offered, not shown; the count holds the session's id, which always shows.
    expect(table.queryByRole("columnheader", { name: /Input tokens/ })).toBeNull();
    expect(screen.getByRole("button", { name: /Columns/ })).toHaveTextContent("8/10");
    // The window and the refresh stand on the title's row, as on the traces page.
    const header = within(screen.getByTestId("page-header"));
    expect(header.getByRole("combobox", { name: "Window" })).toHaveTextContent("Last 7 days");
    expect(header.getByRole("button", { name: "Refresh" })).toBeInTheDocument();
  });

  it("opens a session from anywhere on its row, and marks the row while it is open", async () => {
    render(await SessionsPage({ params: PARAMS }));
    const row = screen.getByRole("link", { name: "conv-1" }).closest("tr")!;
    fireEvent.click(within(row).getByText("1,500"));
    expect(new URL(window.location.href).searchParams.get("peek")).toBe("conv-1");
  });

  it("opens the session the URL names beside the list, read by the project's id in the list's window", async () => {
    mockSearch = "days=30&peek=conv-1";
    const fetchMock = vi.fn(async () => Response.json({ session_id: "conv-1", traces: [], pinned: [], days: 30, limit: 100, more: false }));
    vi.stubGlobal("fetch", fetchMock);
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({ days: "30", limit: "25" }) }));
    const panel = screen.getByRole("dialog");
    expect(panel).toHaveAttribute("aria-modal", "false");
    // The list's rows a page are its own; the session is read whole in the window.
    expect(fetchMock).toHaveBeenCalledWith("/api/v1/sessions/conv-1?project=project_7bQx2mNv9BcK4dLp&days=30", expect.anything());
    expect(await screen.findByText("No session with this id in this project's window.")).toBeInTheDocument();
    vi.unstubAllGlobals();
  });

  it("reads the URL's filters, sort, window and page, hands them to the server, and shows them", async () => {
    render(
      await SessionsPage({
        params: PARAMS,
        searchParams: Promise.resolve({
          sort: "user",
          dir: "desc",
          days: "90",
          limit: "0",
          status: "error",
          page: "2",
        }),
      }),
    );
    expect(fetchSessions).toHaveBeenCalledWith(PROJECT, {
      status: ["error"],
      sort: "user",
      dir: "desc",
      days: 90,
      page: 2,
      limit: 50,
    });
    expect(fetchSessionFacets).toHaveBeenCalledWith(PROJECT, {
      status: ["error"],
      sort: "user",
      dir: "desc",
      days: 90,
      page: 2,
    });
    expect(screen.getByRole("columnheader", { name: /User/ })).toHaveAttribute(
      "aria-sort",
      "descending",
    );
    expect(
      within(screen.getByTestId("page-header")).getByRole("combobox", { name: "Window" }),
    ).toHaveTextContent("Last 90 days");
    // The filter stands above the rows, and its × takes it off from the first page.
    const bar = within(screen.getByLabelText("Filters in force"));
    expect(bar.getByRole("link", { name: "Clear status Error" })).toHaveAttribute(
      "href",
      "/acme/web/sessions?sort=user&dir=desc&days=90",
    );
    // Each value in the panel counts the sessions that hold a run carrying it.
    const filters = within(screen.getByRole("complementary", { name: "Filters" }));
    expect(filters.getByText("support-bot").closest("[cmdk-item]")).toHaveTextContent("2");
  });

  it("pages through the sessions the filters admit, and offers the rows a page holds", async () => {
    mockSessions = list({ sessions: [SESSION_A], total: 120, page: 2 });
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({ page: "2" }) }));
    expect(screen.getByText("Total 120")).toBeInTheDocument();
    expect(screen.getByText("Page 2 of 3")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Next page" })).toHaveAttribute(
      "href",
      "/acme/web/sessions?page=3",
    );
    expect(screen.getByRole("link", { name: "Previous page" })).toHaveAttribute(
      "href",
      "/acme/web/sessions",
    );
    expect(screen.getByRole("combobox", { name: "Rows per page" })).toHaveTextContent("50");
  });

  it("downloads the list as it is filtered and sorted, beside the page rather than under it", async () => {
    render(
      await SessionsPage({
        params: PARAMS,
        searchParams: Promise.resolve({ status: "error", sort: "tokens", page: "2" }),
      }),
    );
    fireEvent.keyDown(screen.getByRole("button", { name: "Download" }), { key: "Enter" });
    expect(await screen.findByText("2 sessions, as the list is sorted")).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "CSV, for a spreadsheet" })).toHaveAttribute(
      "href",
      "/acme/web/session-export?status=error&sort=tokens&format=csv",
    );
  });

  it("shows the columns the person picked, in the table's order", async () => {
    window.localStorage.setItem(
      TRACE_PREFS_KEY,
      JSON.stringify({ sessionColumns: ["output_tokens", "input_tokens"] }),
    );
    render(await SessionsPage({ params: PARAMS }));
    const headings = within(screen.getByRole("table"))
      .getAllByRole("columnheader")
      .map((th) => th.textContent);
    expect(headings).toEqual(["Session", "Input tokens", "Output tokens"]);
    expect(within(screen.getByRole("table")).getByText("1,200")).toBeInTheDocument();
  });

  it("says when the filters admit no session, with the way back to every one", async () => {
    mockSessions = list({ sessions: [], total: 0 });
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({ agent: "billing-bot" }) }));
    expect(screen.getByText("No sessions match these filters.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "clear the filters" })).toHaveAttribute(
      "href",
      "/acme/web/sessions",
    );
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("tells a project that names no session which attribute to set", async () => {
    mockSessions = list({ sessions: [], total: 0 });
    render(await SessionsPage({ params: PARAMS }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.getByText("gen_ai.conversation.id")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "SDKs" })).toHaveAttribute("href", "/docs/api/sdks");
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("sends a refused role back to the project's overview, and says so on an outage", async () => {
    mockSessions = { kind: "forbidden" };
    render(await SessionsPage({ params: PARAMS }));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
      "href",
      "/acme/web",
    );

    mockSessions = { kind: "unavailable" };
    render(await SessionsPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });

  it("says the filters could not be read, and lists the sessions all the same", async () => {
    mockFacets = { kind: "unavailable" };
    render(await SessionsPage({ params: PARAMS }));
    expect(screen.getByText("The filters’ values could not be read.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "conv-1" })).toBeInTheDocument();
  });

  it("renders ServiceUnavailable when context fails", async () => {
    mockContext = null;
    render(await SessionsPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
