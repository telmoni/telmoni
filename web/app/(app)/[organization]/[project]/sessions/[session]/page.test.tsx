// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, backHref }: { title: string; backHref?: string }) => (
    <div data-testid="page-header" data-back={backHref}>
      {title}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

import { Role } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
let mockSession: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchSession = vi.fn<
  (projectId: string, sessionId: string, query: unknown) => Promise<unknown>
>(async () => mockSession);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchSession: (projectId: string, sessionId: string, query: unknown) =>
    fetchSession(projectId, sessionId, query),
}));

import SessionPage from "./page";

// The segment as Next hands it over: decoded, slash and space and all.
const SESSION = "chat/42 b";
const ENCODED = "chat%2F42%20b";
const PARAMS = Promise.resolve({
  organization: "acme",
  project: "web",
  session: SESSION,
});
const HASHED = "9f86d081884c7d659a2feaa0c55ad015";
const RUN_A = "5d4f5c68-0000-4000-8000-000000000001";
const RUN_B = "7a1b2c3d-0000-4000-8000-000000000002";

const TRACE_A = {
  run_id: RUN_A,
  agent: "support-bot",
  user_id: HASHED,
  session_id: SESSION,
  ended: true,
  status: "ok",
  started_at: "2026-10-11T03:00:00Z",
  ended_at: "2026-10-11T03:00:02Z",
  duration_ms: 2000,
  spans: 4,
  steps: 1,
  model_calls: 1,
  tool_calls: 1,
  errors: 0,
  input_tokens: 1200,
  output_tokens: 300,
  cache_read_input_tokens: 100,
  cache_write_input_tokens: 50,
  models: ["claude-sonnet-5-5"],
  tools: ["search_docs"],
};
const TRACE_B = {
  ...TRACE_A,
  run_id: RUN_B,
  // The second run named no user; the session's user is read off the first that did.
  user_id: "",
  status: "error",
  started_at: "2026-10-11T03:01:00Z",
  ended_at: "2026-10-11T03:01:00.500Z",
  duration_ms: 500,
  steps: 2,
  errors: 1,
  input_tokens: 10,
  output_tokens: 5,
};

function detail(over: Record<string, unknown> = {}) {
  return {
    kind: "ok",
    session: {
      session_id: SESSION,
      traces: [TRACE_A, TRACE_B],
      days: 7,
      limit: 100,
      more: false,
      ...over,
    },
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
  mockSession = detail();
});

describe("SessionPage", () => {
  it("sums the session and replays its runs in order, each opening its trace", async () => {
    render(await SessionPage({ params: PARAMS }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Session");
    expect(screen.getByTestId("page-header")).toHaveAttribute("data-back", "/acme/web/sessions");
    expect(fetchSession).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", SESSION, {});

    expect(screen.getByText(SESSION)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: HASHED })).toHaveAttribute(
      "href",
      `/acme/web/traces?user=${HASHED}`,
    );
    expect(screen.getByText("1 in error", { exact: false })).toBeInTheDocument();
    expect(screen.getByText("1,515")).toBeInTheDocument();
    expect(screen.getByText("(1,210 in, 305 out)")).toBeInTheDocument();

    const runs = screen.getAllByRole("link", { name: /^[0-9a-f]{8}$/ });
    expect(runs.map((a) => a.getAttribute("href"))).toEqual([
      `/acme/web/traces/${RUN_A}`,
      `/acme/web/traces/${RUN_B}`,
    ]);
    expect(screen.getAllByText("support-bot")).toHaveLength(2);
    expect(screen.getByText("OK")).toBeInTheDocument();
    expect(screen.getByText("Error")).toBeInTheDocument();
    expect(screen.getByText("2 s")).toBeInTheDocument();
    expect(screen.getByText("500 ms")).toBeInTheDocument();

    // The window form posts back to this session, encoded as the path carries it.
    expect(screen.getByRole("form", { name: "Choose the window" })).toHaveAttribute(
      "action",
      `/acme/web/sessions/${ENCODED}`,
    );
  });

  it("reads the window from the URL, hands it to the server, and offers the larger page", async () => {
    mockSession = detail({ traces: [TRACE_A], more: true });
    render(
      await SessionPage({
        params: PARAMS,
        searchParams: Promise.resolve({ days: "30", limit: "x" }),
      }),
    );
    expect(fetchSession).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", SESSION, {
      days: 30,
    });
    expect(screen.getByText("1 shown", { exact: false })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Show up to 500" })).toHaveAttribute(
      "href",
      `/acme/web/sessions/${ENCODED}?days=30&limit=500`,
    );

    cleanup();
    mockSession = detail({ traces: [TRACE_A], limit: 500, more: true });
    render(await SessionPage({ params: PARAMS, searchParams: Promise.resolve({ limit: "500" }) }));
    expect(screen.getByText(/Narrow the window to see the rest\./)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Show up to 500" })).toBeNull();
  });

  it("says when no run of the window belonged to the session, and leads back", async () => {
    mockSession = { kind: "missing" };
    render(await SessionPage({ params: PARAMS }));
    expect(screen.getByText(/No session with this id/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to the sessions" })).toHaveAttribute(
      "href",
      "/acme/web/sessions",
    );
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("sends a refused role back to the project's overview, and says so on an outage", async () => {
    mockSession = { kind: "forbidden" };
    render(await SessionPage({ params: PARAMS }));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
      "href",
      "/acme/web",
    );

    mockSession = { kind: "unavailable" };
    render(await SessionPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });

  it("renders ServiceUnavailable when context fails", async () => {
    mockContext = null;
    render(await SessionPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
