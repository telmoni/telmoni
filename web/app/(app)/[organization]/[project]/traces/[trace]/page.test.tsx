// @vitest-environment jsdom
import { fireEvent, render, screen, within } from "@testing-library/react";
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

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

// The layout reads the link's part of the URL.
let mockSearch = "";
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/acme/web/traces/5d4f5c68-0000-4000-8000-000000000001",
  useSearchParams: () => new URLSearchParams(mockSearch),
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
let mockTrace: unknown;
let mockSetting: unknown;
let mockLayouts: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchTrace = vi.fn<(projectId: string, runId: string) => Promise<unknown>>(
  async () => mockTrace,
);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchTrace: (projectId: string, runId: string) => fetchTrace(projectId, runId),
  fetchSessionLayout: async () => mockSetting,
  fetchTraceLayouts: async () => mockLayouts,
}));

import TracePage from "./page";

const RUN = "5d4f5c68-0000-4000-8000-000000000001";
const PARAMS = Promise.resolve({ organization: "acme", project: "web", trace: RUN });
const LAYOUT_ID = "0b6f2c1e-6a4f-4d2b-9e3a-5c7d8e9f0a1b";
const FULL_ID = "1c7e3d2f-7b5a-4e3c-8f4b-6d8e9f0a1b2c";
/** The default customized all the way: steps and calls, the failed open on
 *  a timeline. */
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
  columns: ["started"],
  created_at: "2026-10-17T00:00:00Z",
  updated_at: "2026-10-17T00:00:00Z",
};
const PINNING = {
  id: LAYOUT_ID,
  kind: "layout",
  name: "Pinned tools",
  query: {
    depth: "steps",
    open: "none",
    timeline: false,
    pinned: ["search_docs"],
    filter: {},
    messages: false,
    scores: true,
  },
  columns: ["started"],
  created_at: "2026-10-17T00:00:00Z",
  updated_at: "2026-10-17T00:00:00Z",
};

function span(over: Record<string, unknown>) {
  return {
    span_id: "0000000000000001",
    parent_span_id: "",
    trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
    agent: "nightly-report",
    kind: "run",
    name: "nightly-report",
    started_at: "2026-10-11T03:00:00.000Z",
    ended_at: "2026-10-11T03:00:02.000Z",
    status: "ok",
    error_type: "",
    model: "",
    provider: "",
    input_tokens: 0,
    output_tokens: 0,
    cache_read_input_tokens: 0,
    cache_write_input_tokens: 0,
    user_id: "",
    session_id:"",
    environment: "",
    release: "",
    tags: {},
    bound_stall_after_s: null,
    bound_max_steps: null,
    bound_max_cost_usd: null,
    sample_rate: null,
    attributes: {},
    units: 1,
    received_at: "2026-10-11T03:00:02.000Z",
    ...over,
  };
}

const SPANS = [
  span({
    user_id: "9f86d081884c7d659a2feaa0c55ad015",
    session_id:"conv-1",
    tags: { team: "search" },
    bound_max_steps: 40,
  }),
  span({
    span_id: "0000000000000002",
    parent_span_id: "0000000000000001",
    kind: "step",
    name: "plan the reply",
    started_at: "2026-10-11T03:00:00.010Z",
    ended_at: "2026-10-11T03:00:01.910Z",
  }),
  span({
    span_id: "0000000000000003",
    parent_span_id: "0000000000000002",
    kind: "model_call",
    name: "chat claude-sonnet-5-5",
    model: "claude-sonnet-5-5",
    provider: "anthropic",
    input_tokens: 1200,
    output_tokens: 300,
    started_at: "2026-10-11T03:00:00.020Z",
    ended_at: "2026-10-11T03:00:01.520Z",
  }),
  span({
    span_id: "0000000000000004",
    parent_span_id: "0000000000000002",
    kind: "tool_call",
    name: "search_docs",
    status: "error",
    error_type: "TimeoutError",
    started_at: "2026-10-11T03:00:01.600Z",
    ended_at: "2026-10-11T03:00:01.800Z",
  }),
];

beforeEach(() => {
  vi.clearAllMocks();
  mockSearch = "";
  window.localStorage.clear();
  // The project opens in a layout of steps and calls, as the tests below
  // read the whole tree; the default is tested on its own.
  mockSetting = { view_id: FULL_ID };
  mockLayouts = { kind: "ok", layouts: [PINNING, FULL] };
  mockContext = {
    person: { userId: "user_1", email: "ada@example.test", analyticsOptIn: false },
    organizations: [],
    activeOrganizationId: "org_1",
    memberships: [],
    incomingInvites: [],
    flags: {},
  };
  mockProject = { id: "project_7bQx2mNv9BcK4dLp", slug: "web", name: "Web", role: Role.Member };
  mockTrace = { kind: "ok", trace: { run_id: RUN, spans: SPANS, truncated: false } };
});

describe("TracePage", () => {
  // Four days after the run, inside the week the user's and the session's
  // pages open on, so their links need no window of their own.
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date("2026-10-15T12:00:00.000Z"));
    // A menu's popper measures its trigger, which jsdom cannot.
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        unobserve() {}
        disconnect() {}
      },
    );
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  const chosen = () => screen.getByRole("region", { name: "The span chosen" });

  it("names the run, sums it on one line, and opens on the run's own span", async () => {
    render(await TracePage({ params: PARAMS }));
    expect(fetchTrace).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", RUN);
    const header = screen.getByTestId("page-header");
    expect(header).toHaveTextContent("nightly-report");
    expect(header).toHaveAttribute("data-back", "/acme/web/traces");

    // The line under the name: how long, the model calls' tokens, and the
    // session and the user it carried, each leading to its page.
    expect(screen.getAllByText("2 s").length).toBeGreaterThan(0);
    expect(
      screen.getAllByText("1,500 tokens").some((el) => el.getAttribute("title") === "1,200 in, 300 out"),
    ).toBe(true);
    expect(screen.getByRole("link", { name: "session" })).toHaveAttribute(
      "href",
      "/acme/web/sessions/conv-1",
    );
    expect(screen.getByRole("link", { name: "user 9f86d081" })).toHaveAttribute(
      "href",
      "/acme/web/traces?user=9f86d081884c7d659a2feaa0c55ad015",
    );

    // The spans as they ran: the run, its step, and the step's two calls.
    const tree = screen.getByRole("tree", { name: "Spans" });
    const rows = within(tree).getAllByRole("treeitem");
    expect(rows.map((row) => row.getAttribute("aria-level"))).toEqual(["1", "2", "3", "3"]);
    expect(rows[0]).toHaveAttribute("aria-selected", "true");
    expect(within(tree).getByText("TimeoutError")).toBeInTheDocument();

    // The run's own span, read with the whole trace's sums.
    const run = chosen();
    expect(within(run).getByRole("heading", { name: "nightly-report" })).toBeInTheDocument();
    expect(within(run).getByText("40 steps at most")).toBeInTheDocument();
    expect(within(run).getByText(/1 step, 2 calls/)).toBeInTheDocument();
    expect(within(run).getByText("(1,200 in, 300 out)")).toBeInTheDocument();
    expect(within(run).getAllByText("team=search").length).toBeGreaterThan(0);
    expect(within(run).getByText(RUN)).toBeInTheDocument();
  });

  it("reads the span chosen in the tree: its model and tokens, and how it failed", async () => {
    render(await TracePage({ params: PARAMS }));
    const tree = screen.getByRole("tree", { name: "Spans" });

    fireEvent.click(within(tree).getByText("search_docs"));
    expect(within(chosen()).getByRole("heading", { name: "search_docs" })).toBeInTheDocument();
    expect(within(chosen()).getAllByText("Error: TimeoutError").length).toBeGreaterThan(0);
    expect(within(chosen()).getAllByText("200 ms").length).toBeGreaterThan(0);

    fireEvent.click(within(tree).getByText("chat claude-sonnet-5-5"));
    expect(within(chosen()).getAllByRole("link", { name: "claude-sonnet-5-5" })[0]).toHaveAttribute(
      "href",
      "/acme/web/traces?model=claude-sonnet-5-5",
    );
    expect(within(chosen()).getByText("1,200")).toBeInTheDocument();
    expect(within(chosen()).getByText("anthropic")).toBeInTheDocument();
  });

  it("walks the tree from the keyboard, the focus with the span chosen, and shuts a branch and opens it again", async () => {
    // The frame the row waits for, to take the focus, comes at once.
    vi.stubGlobal("requestAnimationFrame", (step: FrameRequestCallback) => {
      step(0);
      return 0;
    });
    render(await TracePage({ params: PARAMS }));
    const tree = screen.getByRole("tree", { name: "Spans" });
    const row = (name: string) =>
      within(tree)
        .getAllByRole("treeitem")
        .find((item) => item.textContent?.includes(name))!;

    fireEvent.keyDown(row("nightly-report"), { key: "ArrowDown" });
    expect(within(chosen()).getByRole("heading", { name: "plan the reply" })).toBeInTheDocument();
    expect(document.activeElement).toBe(row("plan the reply"));
    fireEvent.keyDown(row("plan the reply"), { key: "ArrowLeft" });
    expect(within(tree).getAllByRole("treeitem")).toHaveLength(2);
    fireEvent.keyDown(row("plan the reply"), { key: "ArrowRight" });
    expect(within(tree).getAllByRole("treeitem")).toHaveLength(4);
    fireEvent.keyDown(row("plan the reply"), { key: "End" });
    expect(within(chosen()).getByRole("heading", { name: "search_docs" })).toBeInTheDocument();
    expect(document.activeElement).toBe(row("search_docs"));
    // The two calls are a set of two under their step, said outright.
    expect(row("search_docs")).toHaveAttribute("aria-posinset", "2");
    expect(row("search_docs")).toHaveAttribute("aria-setsize", "2");
  });

  it("chooses the branch it shuts when the span chosen was inside it", async () => {
    render(await TracePage({ params: PARAMS }));
    const tree = screen.getByRole("tree", { name: "Spans" });
    fireEvent.click(within(tree).getByText("search_docs"));
    const step = within(tree)
      .getAllByRole("treeitem")
      .find((item) => item.textContent?.includes("plan the reply"))!;
    fireEvent.click(step.querySelector(".lucide-chevron-right")!.parentElement!);
    expect(within(tree).getAllByRole("treeitem")).toHaveLength(2);
    expect(within(chosen()).getByRole("heading", { name: "plan the reply" })).toBeInTheDocument();
    expect(step).toHaveAttribute("aria-selected", "true");
  });

  it("finds spans by a search, and draws the timeline a toggle away, every span a bar", async () => {
    render(await TracePage({ params: PARAMS }));
    fireEvent.change(screen.getByRole("searchbox", { name: "Search spans" }), {
      target: { value: "search_docs" },
    });
    const tree = screen.getByRole("tree", { name: "Spans" });
    // The match with the spans above it.
    expect(within(tree).getAllByRole("treeitem")).toHaveLength(3);
    // Escape empties the search before it would shut anything.
    const field = screen.getByRole("searchbox", { name: "Search spans" });
    expect(fireEvent.keyDown(field, { key: "Escape" })).toBe(false);
    expect(field).toHaveValue("");
    expect(within(tree).getAllByRole("treeitem")).toHaveLength(4);

    fireEvent.click(screen.getByRole("radio", { name: "Timeline" }));
    expect(tree.querySelectorAll('[role="treeitem"] span[style*="width"]')).toHaveLength(4);
  });

  it("opens on the run's own row alone by default, and names the layout as changed once customized", async () => {
    mockSetting = { view_id: null };
    render(await TracePage({ params: PARAMS }));
    expect(screen.getByRole("button", { name: "Customize the layout" })).toBeInTheDocument();
    const tree = screen.getByRole("tree", { name: "Spans" });
    expect(within(tree).getAllByRole("treeitem")).toHaveLength(1);
    // The run's own span, chosen, still reads with the whole trace's sums.
    expect(within(chosen()).getByText(/1 step, 2 calls/)).toBeInTheDocument();
    // Customized to its steps, the layout is named as changed. The menu opens
    // from the keyboard, as Radix opens one in a document with no pointer.
    fireEvent.keyDown(screen.getByRole("button", { name: "Customize the layout" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitemradio", { name: "Its steps" }));
    expect(within(screen.getByRole("tree", { name: "Spans" })).getAllByRole("treeitem")).toHaveLength(2);
    expect(screen.getByRole("button", { name: "Customize the layout, changed" })).toBeInTheDocument();
  });

  it("opens a saved layout from a link, its pinned spans above the tree, chosen at a press", async () => {
    mockSearch = `view=${LAYOUT_ID}`;
    render(await TracePage({ params: PARAMS }));
    expect(screen.getByRole("button", { name: "Saved layout: Pinned tools" })).toBeInTheDocument();
    const tree = screen.getByRole("tree", { name: "Spans" });
    expect(within(tree).getAllByRole("treeitem")).toHaveLength(2);
    const pin = screen.getByRole("button", { name: /search_docs/ });
    fireEvent.click(pin);
    expect(pin).toHaveAttribute("aria-pressed", "true");
    expect(within(chosen()).getByRole("heading", { name: "search_docs" })).toBeInTheDocument();
  });

  it("calls a run whose own span has not arrived running", async () => {
    mockTrace = { kind: "ok", trace: { run_id: RUN, spans: SPANS.slice(2, 3), truncated: true } };
    render(await TracePage({ params: PARAMS }));
    expect(screen.getByText("Running")).toBeInTheDocument();
    expect(within(screen.getByRole("tree")).getByText("chat claude-sonnet-5-5")).toBeInTheDocument();
    expect(screen.getByText(/more spans than the server sends at once/)).toBeInTheDocument();
  });

  it("says when there is no such trace here, and leads back to the list", async () => {
    mockTrace = { kind: "missing" };
    render(await TracePage({ params: PARAMS }));
    expect(screen.getByText("No trace with this id in this project.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to the traces" })).toHaveAttribute(
      "href",
      "/acme/web/traces",
    );
  });

  it("sends a refused role back to the overview, and says so on an outage", async () => {
    mockTrace = { kind: "forbidden" };
    render(await TracePage({ params: PARAMS }));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
      "href",
      "/acme/web",
    );
    mockTrace = { kind: "unavailable" };
    render(await TracePage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
