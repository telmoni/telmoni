// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, count }: { title: string; count?: number }) => (
    <div data-testid="page-header">
      {title}
      {count !== undefined && <span data-testid="count">{count}</span>}
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

import { Role } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
let mockSessions: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchSessions = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(
  async () => mockSessions,
);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchSessions: (projectId: string, query: unknown) => fetchSessions(projectId, query),
  fetchSessionLayout: async () => ({ view_id: null }),
  fetchTraceLayouts: async () => ({ kind: "ok", layouts: [] }),
}));

import SessionsPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });
const HASHED = "9f86d081884c7d659a2feaa0c55ad015";

const SESSION_A = {
  session_id:"conv-1",
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
  session_id:"chat/42 b",
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
      limit: 100,
      more: false,
      sort: "last_seen",
      dir: "desc",
      ...over,
    },
  };
}

beforeEach(() => {
  vi.clearAllMocks();
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
  mockProject = { id: "project_7bQx2mNv9BcK4dLp", slug: "web", name: "Web", role: Role.Member };
  mockSessions = list();
});

describe("SessionsPage", () => {
  it("lists each session with its user and sums, the session opening beside the list — its page from a new tab — and the user their traces", async () => {
    render(await SessionsPage({ params: PARAMS }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Sessions");
    expect(screen.getByTestId("count")).toHaveTextContent("2");
    expect(fetchSessions).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", {});

    expect(screen.getByRole("link", { name: "conv-1" })).toHaveAttribute(
      "href",
      "/acme/web/sessions/conv-1",
    );
    expect(screen.getByRole("link", { name: "chat/42 b" })).toHaveAttribute(
      "href",
      "/acme/web/sessions/chat%2F42%20b",
    );
    expect(screen.getByRole("link", { name: HASHED })).toHaveAttribute(
      "href",
      `/acme/web/traces?user=${HASHED}`,
    );
    // A plain click opens the session beside the list, in the URL; a click
    // for a new tab keeps the link's page.
    fireEvent.click(screen.getByRole("link", { name: "chat/42 b" }));
    expect(new URL(window.location.href).searchParams.get("peek")).toBe("chat/42 b");
    expect(screen.getByRole("link", { name: "conv-1" })).toHaveAttribute("data-peek-session", "conv-1");
    // A session whose runs named no user shows a dash, not a link.
    expect(screen.getByText("—")).toBeInTheDocument();
    expect(screen.getByText("1,500")).toHaveAttribute("title", "1,200 in, 300 out");

    // Last heard from first by default; a click turns it round, and another column starts its own way.
    expect(screen.getByRole("columnheader", { name: /Last seen/ })).toHaveAttribute(
      "aria-sort",
      "descending",
    );
    expect(screen.getByRole("link", { name: /Last seen/ })).toHaveAttribute(
      "href",
      "/acme/web/sessions?sort=last_seen&dir=asc",
    );
    expect(screen.getByRole("link", { name: "Session" })).toHaveAttribute(
      "href",
      "/acme/web/sessions?sort=session&dir=asc",
    );
    expect(screen.getByRole("link", { name: "Errors" })).toHaveAttribute(
      "href",
      "/acme/web/sessions?sort=errors&dir=desc",
    );
  });

  it("opens the session the URL names beside the list, read by the project's id in the list's window", async () => {
    mockSearch = "days=30&peek=conv-1";
    const fetchMock = vi.fn(async () => Response.json({ session_id: "conv-1", traces: [], pinned: [], days: 30, limit: 100, more: false }));
    vi.stubGlobal("fetch", fetchMock);
    render(await SessionsPage({ params: PARAMS, searchParams: Promise.resolve({ days: "30" }) }));
    const panel = screen.getByRole("dialog");
    expect(panel).toHaveAttribute("aria-modal", "false");
    expect(fetchMock).toHaveBeenCalledWith("/api/v1/sessions/conv-1?project=project_7bQx2mNv9BcK4dLp&days=30", expect.anything());
    expect(await screen.findByText("No session with this id in this project's window.")).toBeInTheDocument();
    vi.unstubAllGlobals();
  });

  it("reads the URL's sort and window, hands them to the server, and shows the window picked", async () => {
    render(
      await SessionsPage({
        params: PARAMS,
        searchParams: Promise.resolve({ sort: "user", dir: "desc", days: "90", limit: "0" }),
      }),
    );
    expect(fetchSessions).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", {
      sort: "user",
      dir: "desc",
      days: 90,
    });
    expect(screen.getByRole("columnheader", { name: /User/ })).toHaveAttribute(
      "aria-sort",
      "descending",
    );
    expect(screen.getByRole("combobox", { name: "Window" })).toHaveTextContent("Last 90 days");
  });

  it("says when the window holds more, and offers the larger page", async () => {
    mockSessions = list({ sessions: [SESSION_A], more: true });
    render(await SessionsPage({ params: PARAMS }));
    expect(screen.queryByTestId("count")).toBeNull();
    expect(screen.getByRole("link", { name: "Show up to 500" })).toHaveAttribute(
      "href",
      "/acme/web/sessions?limit=500",
    );
  });

  it("tells a project that names no session which attribute to set", async () => {
    mockSessions = list({ sessions: [] });
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

  it("renders ServiceUnavailable when context fails", async () => {
    mockContext = null;
    render(await SessionsPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
