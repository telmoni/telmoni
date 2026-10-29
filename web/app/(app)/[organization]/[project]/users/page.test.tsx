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

// The window picker reads and writes the URL through the router.
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/acme/web/users",
  useSearchParams: () => new URLSearchParams(),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

import { TRACE_PREFS_KEY } from "@/lib/trace-prefs";
import { Role } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
let mockUsers: unknown;
let mockFacets: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchUsers = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(
  async () => mockUsers,
);
const fetchUserFacets = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(
  async () => mockFacets,
);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchUsers: (projectId: string, query: unknown) => fetchUsers(projectId, query),
  fetchUserFacets: (projectId: string, query: unknown) => fetchUserFacets(projectId, query),
  fetchUserViews: async () => ({ kind: "ok", views: [] }),
}));

import UsersPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });
const PROJECT = "project_7bQx2mNv9BcK4dLp";
const HASHED = "9f86d081884c7d659a2feaa0c55ad015";

const USER_A = {
  user_id: HASHED,
  traces: 12,
  errors: 2,
  steps: 40,
  input_tokens: 1200,
  output_tokens: 300,
  duration_ms: 1500,
  first_seen: "2026-10-04T03:00:00Z",
  last_seen: "2026-10-11T03:00:02Z",
};
const USER_B = {
  user_id: "user-b",
  traces: 1,
  errors: 0,
  steps: 3,
  input_tokens: 10,
  output_tokens: 5,
  duration_ms: 1500,
  first_seen: "2026-10-10T03:00:00Z",
  last_seen: "2026-10-10T03:00:01Z",
};

function list(over: Record<string, unknown> = {}) {
  return {
    kind: "ok",
    list: {
      users: [USER_A, USER_B],
      days: 7,
      limit: 50,
      page: 1,
      total: 2,
      more: false,
      sort: "tokens",
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
    person: { userId: "user_1", email: "ada@example.test", analyticsOptIn: false },
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
      agent: { values: [{ value: "support-bot", count: 2 }], more: false },
      user: NONE,
      session: NONE,
      status: { values: [{ value: "ok", count: 2 }], more: false },
      model: { values: [{ value: "claude-sonnet-5-5", count: 2 }], more: false },
      tool: NONE,
      environment: NONE,
      release: NONE,
      tag: NONE,
      prompt: NONE,
    },
  };
});

describe("UsersPage", () => {
  it("lists a page of users with their traces, errors, steps and tokens, each opening their traces", async () => {
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Users");
    expect(screen.getByTestId("page-header").dataset.info).toMatch(/^Everyone this project's agents served/);
    expect(fetchProjectBySlug).toHaveBeenCalledWith("web");
    // A page holds fifty rows unless the URL names another count.
    expect(fetchUsers).toHaveBeenCalledWith(PROJECT, { limit: 50 });
    expect(fetchUserFacets).toHaveBeenCalledWith(PROJECT, {});
    expect(screen.getByText("Total 2")).toBeInTheDocument();

    const table = within(screen.getByRole("table"));
    expect(table.getByRole("link", { name: HASHED })).toHaveAttribute(
      "href",
      `/acme/web/traces?user=${HASHED}`,
    );
    expect(table.getByRole("link", { name: "user-b" })).toHaveAttribute(
      "href",
      "/acme/web/traces?user=user-b",
    );
    expect(table.getByText("12")).toBeInTheDocument();
    expect(table.getByText("40")).toBeInTheDocument();
    expect(table.getByText("1,500")).toHaveAttribute("title", "1,200 in, 300 out");

    // Most tokens first by default; a click turns it round, and another column starts its own way.
    expect(table.getByRole("columnheader", { name: /^Tokens/ })).toHaveAttribute("aria-sort", "descending");
    expect(table.getByRole("link", { name: /^Tokens/ })).toHaveAttribute(
      "href",
      "/acme/web/users?sort=tokens&dir=asc",
    );
    expect(table.getByRole("link", { name: "User" })).toHaveAttribute(
      "href",
      "/acme/web/users?sort=user&dir=asc",
    );
    expect(table.getByRole("link", { name: "Last seen" })).toHaveAttribute(
      "href",
      "/acme/web/users?sort=last_seen&dir=desc",
    );
    expect(screen.getByRole("button", { name: /Columns/ })).toHaveTextContent("8/10");
    // The window and the refresh stand on the title's row, as on the traces page.
    const header = within(screen.getByTestId("page-header"));
    expect(header.getByRole("combobox", { name: "Window" })).toHaveTextContent("Last 7 days");
    expect(header.getByRole("button", { name: "Refresh" })).toBeInTheDocument();
  });

  it("reads the URL's filters, sort, window and page, hands them to the server, and shows them", async () => {
    render(
      await UsersPage({
        params: PARAMS,
        searchParams: Promise.resolve({
          sort: "first_seen",
          days: "30",
          limit: "9999",
          dir: "down",
          model: "claude-sonnet-5-5",
        }),
      }),
    );
    expect(fetchUsers).toHaveBeenCalledWith(PROJECT, {
      model: ["claude-sonnet-5-5"],
      sort: "first_seen",
      days: 30,
      limit: 50,
    });
    expect(screen.getByRole("columnheader", { name: /First seen/ })).toHaveAttribute(
      "aria-sort",
      "ascending",
    );
    expect(screen.getByRole("link", { name: /First seen/ })).toHaveAttribute(
      "href",
      "/acme/web/users?model=claude-sonnet-5-5&sort=first_seen&days=30&dir=desc",
    );
    expect(
      within(screen.getByTestId("page-header")).getByRole("combobox", { name: "Window" }),
    ).toHaveTextContent("Last 30 days");
    const bar = within(screen.getByLabelText("Filters in force"));
    expect(bar.getByRole("link", { name: "Clear all" })).toHaveAttribute(
      "href",
      "/acme/web/users?sort=first_seen&days=30",
    );
    // Each value in the panel counts the users with a run carrying it.
    const filters = within(screen.getByRole("complementary", { name: "Filters" }));
    expect(filters.getByText("claude-sonnet-5-5").closest("[cmdk-item]")).toHaveTextContent("2");
  });

  it("pages through the users the filters admit", async () => {
    mockUsers = list({ users: [USER_B], total: 51, page: 2 });
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({ page: "2" }) }));
    expect(screen.getByText("Total 51")).toBeInTheDocument();
    expect(screen.getByText("Page 2 of 2")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "First page" })).toHaveAttribute("href", "/acme/web/users");
    expect(screen.getByRole("button", { name: "Next page" })).toBeDisabled();
  });

  it("offers a page past the last its way back", async () => {
    mockUsers = list({ users: [], total: 2, page: 4 });
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({ page: "4" }) }));
    expect(screen.getByText("This page is past the last one.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Go to the last page" })).toHaveAttribute(
      "href",
      "/acme/web/users",
    );
  });

  it("downloads the list as it is filtered and sorted", async () => {
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({ agent: "support-bot" }) }));
    fireEvent.keyDown(screen.getByRole("button", { name: "Download" }), { key: "Enter" });
    expect(await screen.findByText("2 users, as the list is sorted")).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "JSON" })).toHaveAttribute(
      "href",
      "/acme/web/users/export?agent=support-bot&format=json",
    );
  });

  it("shows the columns the person picked, and puts them back to the defaults", async () => {
    window.localStorage.setItem(TRACE_PREFS_KEY, JSON.stringify({ userColumns: ["errors"] }));
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const headings = () =>
      within(screen.getByRole("table"))
        .getAllByRole("columnheader")
        .map((th) => th.textContent);
    expect(headings()).toEqual(["User", "Errors"]);
    fireEvent.keyDown(screen.getByRole("button", { name: /^Columns/ }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Back to the defaults" }));
    expect(headings()).toEqual(["User", "Traces", "Errors", "Steps", "Tokens", "Duration", "First seen", "Last seen"]);
  });

  it("says when the filters admit no user, and tells a project that names none which attribute to set", async () => {
    mockUsers = list({ users: [], total: 0 });
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({ status: "error" }) }));
    expect(screen.getByText("No users match these filters.")).toBeInTheDocument();

    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.getByText("user.id")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "SDKs" })).toHaveAttribute("href", "/docs/api/sdks");
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("sends a refused role back to the project's overview, and says so on an outage", async () => {
    mockUsers = { kind: "forbidden" };
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
      "href",
      "/acme/web",
    );

    mockUsers = { kind: "unavailable" };
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });

  it("renders ServiceUnavailable when context fails", async () => {
    mockContext = null;
    render(await UsersPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
