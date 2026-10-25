// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({
    title,
    backHref,
    action,
  }: {
    title: string;
    backHref?: string;
    action?: React.ReactNode;
  }) => (
    <div data-testid="page-header" data-back={backHref}>
      {title}
      {action}
    </div>
  ),
}));

// The window picker reads and writes the URL through the router; the layout
// reads the link's part of it.
let mockSearch = "";
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/acme/web/sessions/conv-1",
  useSearchParams: () => new URLSearchParams(mockSearch),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

vi.mock("@/app/(app)/[organization]/[project]/traces/actions", () => ({
  saveTraceViewAction: vi.fn(),
  updateTraceViewAction: vi.fn(),
  deleteTraceViewAction: vi.fn(),
  setSessionLayoutAction: vi.fn(),
}));

import { Role } from "@/lib/types/enums";
import { pageSteps } from "@/lib/use-page-steps";

let mockContext: unknown;
let mockProject: unknown;
let mockSession: unknown;
let mockSetting: unknown;
let mockLayouts: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchSession = vi.fn<
  (projectId: string, sessionId: string, query: unknown, pinned: unknown) => Promise<unknown>
>(async () => mockSession);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchSession: (projectId: string, sessionId: string, query: unknown, pinned: unknown) =>
    fetchSession(projectId, sessionId, query, pinned),
  fetchSessionLayout: async () => mockSetting,
  fetchTraceLayouts: async () => mockLayouts,
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
const LAYOUT_ID = "0b6f2c1e-6a4f-4d2b-9e3a-5c7d8e9f0a1b";

const TRACE_A = {
  run_id: RUN_A,
  agent: "support-bot",
  user_id: HASHED,
  session_id: SESSION,
  ended: true,
  status: "ok",
  error_type: "",
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
  environments: [],
  releases: [],
  pinned: [],
};
const TRACE_B = {
  ...TRACE_A,
  run_id: RUN_B,
  // The second run named no user; the session's user is read off the first that did.
  user_id: "",
  status: "error",
  error_type: "RuntimeError",
  started_at: "2026-10-11T03:01:00Z",
  ended_at: "2026-10-11T03:01:00.500Z",
  duration_ms: 500,
  steps: 2,
  errors: 1,
  input_tokens: 10,
  output_tokens: 5,
};

const PINNED = {
  span_id: "0000000000000002",
  parent_span_id: "0000000000000001",
  kind: "step",
  name: "answer",
  started_at: "2026-10-11T03:00:00.010Z",
  ended_at: "2026-10-11T03:00:01.900Z",
  status: "ok",
  error_type: "",
  model: "",
  input_tokens: 0,
  output_tokens: 0,
};

const FULL_ID = "1c7e3d2f-7b5a-4e3c-8f4b-6d8e9f0a1b2c";
/** The default customized all the way: steps and calls, the failed open on
 *  a timeline, with the agent among its columns. */
const FULL = {
  id: FULL_ID,
  kind: "layout",
  name: "Full",
  query: {
    depth: "calls",
    open: "failed",
    timeline: true,
    pinned: [],
    filter: {},
    messages: false,
    scores: true,
  },
  columns: ["started", "agent", "status", "error_type", "tokens", "duration"],
  created_at: "2026-10-17T00:00:00Z",
  updated_at: "2026-10-17T00:00:00Z",
};

const REVIEW = {
  id: LAYOUT_ID,
  kind: "layout",
  name: "Review",
  query: {
    depth: "steps",
    open: "none",
    timeline: false,
    pinned: ["answer"],
    filter: {},
    messages: false,
    scores: true,
  },
  columns: ["started", "agent", "status"],
  created_at: "2026-10-17T00:00:00Z",
  updated_at: "2026-10-17T00:00:00Z",
};

function detail(over: Record<string, unknown> = {}) {
  return {
    kind: "ok",
    session: {
      session_id: SESSION,
      traces: [TRACE_A, TRACE_B],
      pinned: [],
      days: 7,
      limit: 100,
      more: false,
      ...over,
    },
  };
}

/** A trace's spans as the row reads them when opened: the run, a step and
 *  a failed tool call inside it. */
function spans(run: string) {
  const base = {
    parent_span_id: "",
    trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
    agent: "support-bot",
    status: "ok",
    error_type: "",
    model: "",
    provider: "",
    input_tokens: 0,
    output_tokens: 0,
    cache_read_input_tokens: 0,
    cache_write_input_tokens: 0,
    user_id: "",
    session_id: SESSION,
    environment: "",
    release: "",
    tags: {},
    sample_rate: null,
    attributes: {},
    units: 1,
    received_at: "2026-10-11T03:00:02.000Z",
    started_at: "2026-10-11T03:00:00.000Z",
    ended_at: "2026-10-11T03:00:02.000Z",
  };
  return {
    run_id: run,
    truncated: false,
    spans: [
      { ...base, span_id: "1", kind: "run", name: "support-bot" },
      { ...base, span_id: "2", kind: "step", name: "answer", parent_span_id: "1", started_at: "2026-10-11T03:00:00.010Z" },
      {
        ...base,
        span_id: "3",
        kind: "tool_call",
        name: "search_docs",
        parent_span_id: "2",
        status: "error",
        error_type: "TimeoutError",
        started_at: "2026-10-11T03:00:00.020Z",
      },
    ],
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  mockSearch = "";
  window.localStorage.clear();
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
  mockSetting = { view_id: null };
  mockLayouts = { kind: "ok", layouts: [REVIEW, FULL] };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) =>
      Response.json(spans(decodeURIComponent(url.split("/api/v1/traces/")[1]!.split("?")[0]!))),
    ),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("SessionPage", () => {
  it("sums the session and replays its runs in order as turns of a conversation, each opening its trace", async () => {
    render(await SessionPage({ params: PARAMS }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Session");
    expect(screen.getByTestId("page-header")).toHaveAttribute("data-back", "/acme/web/sessions");
    expect(fetchSession).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", SESSION, {}, []);

    expect(screen.getByText(SESSION)).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: HASHED })[0]).toHaveAttribute(
      "href",
      `/acme/web/traces?user=${HASHED}`,
    );
    expect(screen.getByText("1 with failed spans", { exact: false })).toBeInTheDocument();
    expect(screen.getByText("1,515")).toBeInTheDocument();
    expect(screen.getByText("(1,210 in, 305 out)")).toBeInTheDocument();

    const runs = screen.getAllByRole("link", { name: /^[0-9a-f]{8}$/ });
    expect(runs.map((a) => a.getAttribute("href"))).toEqual([
      `/acme/web/traces/${RUN_A}`,
      `/acme/web/traces/${RUN_B}`,
    ]);
    // The default layout: when it started, the user, its status and the
    // class of the error it ended on, duration, tokens, the models and
    // tools by name — and not the agent, which a customized layout may show.
    expect(screen.getByRole("button", { name: "Customize the layout" })).toBeInTheDocument();
    const table = screen.getByRole("table");
    expect(within(table).getAllByRole("columnheader").map((h) => h.textContent)).toEqual([
      "",
      "Trace",
      "Started",
      "User",
      "Status",
      "Error",
      "Tokens",
      "Duration",
      "Models",
      "Tools",
    ]);
    expect(within(table).getByText("OK")).toBeInTheDocument();
    // The heading, and the failed run's status.
    expect(within(table).getAllByText("Error")).toHaveLength(2);
    expect(within(table).getByText("RuntimeError")).toBeInTheDocument();
    expect(within(table).getByText("2 s")).toBeInTheDocument();
    expect(within(table).getByText("500 ms")).toBeInTheDocument();
    expect(within(table).getAllByRole("link", { name: "claude-sonnet-5-5" })[0]).toHaveAttribute(
      "href",
      "/acme/web/traces?model=claude-sonnet-5-5",
    );
    // A trace's line alone: nothing to open, and nothing read.
    expect(within(table).queryByRole("button", { name: "Open the trace" })).toBeNull();
    expect(fetch).not.toHaveBeenCalled();

    // The window picker stands at the session's window; a pick rewrites this
    // page's own address, read from the router, so the encoded id carries over.
    expect(screen.getByRole("combobox", { name: "Window" })).toHaveTextContent("Last 7 days");
  });

  it("opens a layout of steps and calls from a link, the failed trace open, and o shuts it", async () => {
    mockSearch = `view=${FULL_ID}`;
    render(await SessionPage({ params: PARAMS, searchParams: Promise.resolve({ view: FULL_ID }) }));
    expect(screen.getByRole("button", { name: "Saved layout: Full" })).toBeInTheDocument();
    const table = screen.getByRole("table");
    expect(within(table).getAllByText("support-bot").length).toBeGreaterThanOrEqual(2);
    // The failed trace starts open and reads its spans; the other stays shut.
    expect(within(table).getByRole("button", { name: "Shut the trace" })).toBeInTheDocument();
    expect(within(table).getByRole("button", { name: "Open the trace" })).toBeInTheDocument();
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(vi.mocked(fetch).mock.calls[0]![0]).toBe(
      `/api/v1/traces/${RUN_B}?project=project_7bQx2mNv9BcK4dLp`,
    );
    const tree = await within(table).findByRole("list", { name: "Spans" });
    const rows = within(tree).getAllByRole("listitem");
    expect(rows.map((r) => r.textContent)).toEqual([
      expect.stringContaining("answer"),
      expect.stringContaining("search_docs"),
    ]);
    expect(within(tree).getByText("TimeoutError")).toBeInTheDocument();
    expect(tree.querySelectorAll('[role="listitem"] span[style*="width"]')).toHaveLength(2);

    // `j` stands on the first trace and `o` opens it; `j` again, and `o`
    // shuts the failed one: the steps the console's key handler calls.
    act(() => pageSteps()?.next());
    act(() => pageSteps()?.open?.());
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
    expect(within(table).getAllByRole("row")[1]).toHaveAttribute("aria-current", "true");
    act(() => pageSteps()?.next());
    act(() => pageSteps()?.open?.());
    expect(within(table).getAllByRole("button", { name: "Shut the trace" })).toHaveLength(1);
    expect(within(table).getAllByRole("row")[1]).not.toHaveAttribute("aria-current");
  });

  it("opens a saved layout from a link as it was saved, its pinned spans read with the session", async () => {
    mockSearch = `view=${LAYOUT_ID}`;
    mockSession = detail({ pinned: ["answer"], traces: [{ ...TRACE_A, pinned: [PINNED] }, TRACE_B] });
    render(await SessionPage({ params: PARAMS, searchParams: Promise.resolve({ view: LAYOUT_ID }) }));
    expect(fetchSession).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", SESSION, {}, ["answer"]);
    expect(screen.getByRole("button", { name: "Saved layout: Review" })).toBeInTheDocument();
    const table = screen.getByRole("table");
    expect(within(table).getAllByRole("columnheader").map((h) => h.textContent)).toEqual([
      "",
      "Trace",
      "Started",
      "Agent",
      "Status",
    ]);
    const pinned = within(table).getByRole("list", { name: "Pinned spans" });
    expect(within(pinned).getByText("answer")).toBeInTheDocument();
    expect(within(pinned).getByText("1.9 s")).toBeInTheDocument();
    // As saved, so not marked as changed.
    expect(screen.getByRole("button", { name: "Customize the layout" })).toBeInTheDocument();
  });

  it("opens in the project's default layout where the link names none", async () => {
    mockSetting = { view_id: FULL_ID };
    render(await SessionPage({ params: PARAMS }));
    expect(screen.getByRole("button", { name: "Saved layout: Full" })).toBeInTheDocument();
    expect(screen.getByRole("table")).toHaveTextContent("Agent");
    cleanup();
    mockSetting = { view_id: LAYOUT_ID };
    render(await SessionPage({ params: PARAMS }));
    expect(fetchSession).toHaveBeenLastCalledWith("project_7bQx2mNv9BcK4dLp", SESSION, {}, ["answer"]);
    expect(screen.getByRole("button", { name: "Saved layout: Review" })).toBeInTheDocument();
  });

  it("reads the window from the URL, hands it to the server, and offers the larger page", async () => {
    mockSession = detail({ traces: [TRACE_A], more: true });
    render(
      await SessionPage({
        params: PARAMS,
        searchParams: Promise.resolve({ days: "30", limit: "x" }),
      }),
    );
    expect(fetchSession).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", SESSION, { days: 30 }, []);
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
