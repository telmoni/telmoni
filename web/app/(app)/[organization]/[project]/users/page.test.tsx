// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, count }: { title: string; count?: number }) => (
    <div data-testid="page-header">
      {title}
      {count !== undefined && <span data-testid="count">{count}</span>}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

import { Role } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
let mockUsers: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchUsers = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(
  async () => mockUsers,
);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchUsers: (projectId: string, query: unknown) => fetchUsers(projectId, query),
}));

import UsersPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });
const HASHED = "9f86d081884c7d659a2feaa0c55ad015";

const USER_A = {
  user_id: HASHED,
  traces: 12,
  errors: 2,
  steps: 40,
  input_tokens: 1200,
  output_tokens: 300,
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
  first_seen: "2026-10-10T03:00:00Z",
  last_seen: "2026-10-10T03:00:01Z",
};

function list(over: Record<string, unknown> = {}) {
  return {
    kind: "ok",
    list: { users: [USER_A, USER_B], days: 7, limit: 100, more: false, sort: "tokens", dir: "desc", ...over },
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  mockContext = {
    person: { userId: "user_1", email: "ada@example.test", analyticsOptIn: false },
    organizations: [],
    activeOrganizationId: "org_1",
    memberships: [],
    incomingInvites: [],
    flags: {},
  };
  mockProject = { id: "project_7bQx2mNv9BcK4dLp", slug: "web", name: "Web", role: Role.Member };
  mockUsers = list();
});

describe("UsersPage", () => {
  it("lists each user with their traces, errors, steps and tokens, each opening their traces", async () => {
    render(await UsersPage({ params: PARAMS }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Users");
    expect(screen.getByTestId("count")).toHaveTextContent("2");
    expect(fetchProjectBySlug).toHaveBeenCalledWith("web");
    expect(fetchUsers).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", {});

    expect(screen.getByRole("link", { name: HASHED })).toHaveAttribute(
      "href",
      `/acme/web/traces?user=${HASHED}`,
    );
    expect(screen.getByRole("link", { name: "user-b" })).toHaveAttribute(
      "href",
      "/acme/web/traces?user=user-b",
    );
    expect(screen.getByText("12")).toBeInTheDocument();
    expect(screen.getByText("40")).toBeInTheDocument();
    expect(screen.getByText("1,500")).toHaveAttribute("title", "1,200 in, 300 out");

    // Most tokens first by default; a click turns it round, and another column starts its own way.
    expect(screen.getByRole("columnheader", { name: /Tokens/ })).toHaveAttribute("aria-sort", "descending");
    expect(screen.getByRole("link", { name: /Tokens/ })).toHaveAttribute(
      "href",
      "/acme/web/users?sort=tokens&dir=asc",
    );
    expect(screen.getByRole("link", { name: "User" })).toHaveAttribute(
      "href",
      "/acme/web/users?sort=user&dir=asc",
    );
    expect(screen.getByRole("link", { name: "Last seen" })).toHaveAttribute(
      "href",
      "/acme/web/users?sort=last_seen&dir=desc",
    );
  });

  it("reads the URL's sort and window, hands them to the server, and carries them in the form", async () => {
    render(
      await UsersPage({
        params: PARAMS,
        searchParams: Promise.resolve({ sort: "first_seen", days: "30", limit: "9999", dir: "down" }),
      }),
    );
    expect(fetchUsers).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", {
      sort: "first_seen",
      days: 30,
    });
    expect(screen.getByRole("columnheader", { name: /First seen/ })).toHaveAttribute(
      "aria-sort",
      "ascending",
    );
    expect(screen.getByRole("link", { name: /First seen/ })).toHaveAttribute(
      "href",
      "/acme/web/users?sort=first_seen&days=30&dir=desc",
    );
    const form = screen.getByRole("form", { name: "Choose the window" });
    expect(form).toHaveAttribute("action", "/acme/web/users");
    expect(screen.getByRole("combobox", { name: /Window/ })).toHaveValue("30");
    expect(form.querySelector('input[name="sort"]')).toHaveValue("first_seen");
    expect(form.querySelector('input[name="dir"]')).toBeNull();
  });

  it("says when the window holds more, and offers the larger page", async () => {
    mockUsers = list({ users: [USER_A], more: true });
    render(await UsersPage({ params: PARAMS }));
    expect(screen.queryByTestId("count")).toBeNull();
    expect(screen.getByRole("link", { name: "Show up to 500" })).toHaveAttribute(
      "href",
      "/acme/web/users?limit=500",
    );
  });

  it("tells a project that names no user which attribute to set", async () => {
    mockUsers = list({ users: [] });
    render(await UsersPage({ params: PARAMS }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.getByText("user.id")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "SDKs" })).toHaveAttribute("href", "/docs/api/sdks");
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("sends a refused role back to the project's overview, and says so on an outage", async () => {
    mockUsers = { kind: "forbidden" };
    render(await UsersPage({ params: PARAMS }));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
      "href",
      "/acme/web",
    );

    mockUsers = { kind: "unavailable" };
    render(await UsersPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });

  it("renders ServiceUnavailable when context fails", async () => {
    mockContext = null;
    render(await UsersPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
