// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  setTraceTagsAction: vi.fn(),
}));

import { setTraceTagsAction } from "@/app/(app)/[organization]/[project]/traces/actions";
import { testSpan } from "@/lib/span-fixture";
import { Role, SpanKind } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
let mockTrace: unknown;
let mockSetting: unknown;
let mockLayouts: unknown;
let mockIncidents: unknown[];
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchTrace = vi.fn<(projectId: string, runId: string, content: boolean) => Promise<unknown>>(
  async () => mockTrace,
);
const fetchRunIncidents = vi.fn<(projectId: string, runId: string) => Promise<unknown[]>>(
  async () => mockIncidents,
);

const SCORES = vi.hoisted(() => [
  {
    score_id: "5d4f5c68-0000-4000-8000-0000000000a1",
    target: "run",
    name: "helpful",
    data_type: "number",
    value: 0.8,
    categories: [],
    source: "member",
    scorer: "user_1",
    given_at: "2026-11-02T00:00:00Z",
  },
  {
    score_id: "5d4f5c68-0000-4000-8000-0000000000a2",
    target: "run",
    name: "grounded",
    data_type: "boolean",
    value: 1,
    categories: [],
    source: "sdk",
    given_at: "2026-11-02T00:00:00Z",
  },
]);
vi.mock("@/lib/server/entities/scores", () => ({
  fetchScores: async () => ({ kind: "ok", scores: SCORES }),
}));
vi.mock("@/lib/server/entities/content-mode", () => ({
  fetchContentMode: async () => ({ content_mode: "on", offered: ["off", "on"] }),
}));
vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchTrace: (projectId: string, runId: string, content: boolean) => fetchTrace(projectId, runId, content),
  fetchSessionLayout: async () => mockSetting,
  fetchTraceLayouts: async () => mockLayouts,
  fetchRunIncidents: (projectId: string, runId: string) => fetchRunIncidents(projectId, runId),
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
    graph: false,
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
    graph: false,
    pinned: ["search_docs"],
    filter: {},
    messages: false,
    scores: true,
  },
  columns: ["started"],
  created_at: "2026-10-17T00:00:00Z",
  updated_at: "2026-10-17T00:00:00Z",
};

const span = (over: Record<string, unknown>) =>
  testSpan({
    span_id: "0000000000000001",
    kind: SpanKind.Run,
    name: "nightly-report",
    ended_at: "2026-10-11T03:00:02.000Z",
    ...over,
  } as Parameters<typeof testSpan>[0]);

const SPANS = [
  span({
    user_id: "9f86d081884c7d659a2feaa0c55ad015",
    session_id:"conv-1",
    tags: { team: "search" },
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
    time_to_first_token: 0.42,
    attributes: { "gen_ai.request.temperature": "0.2", "gen_ai.request.seed": "7" },
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
  mockTrace = {
    kind: "ok",
    trace: {
      run_id: RUN,
      spans: SPANS,
      truncated: false,
      spend: { spent: 0.84, unpriced: [{ provider: "openai", model: "ft:gpt-4o:acme" }] },
      content: [],
    },
  };
  mockIncidents = [];
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
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(fetchTrace).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", RUN, true);
    const header = screen.getByTestId("page-header");
    expect(header).toHaveTextContent("nightly-report");
    expect(header).toHaveAttribute("data-back", "/acme/web/traces");

    // The line under the name: how long, the model calls' tokens, and the
    // session and the user it carried, each leading to its page.
    expect(screen.getAllByText("2 s").length).toBeGreaterThan(0);
    expect(
      screen.getAllByText("1,500 tokens").some((el) => el.getAttribute("title") === "1,200 in, 300 out"),
    ).toBe(true);
    // ⚠ What the run spent, as its cost alert's notice gives it, and the models left out.
    expect(
      screen
        .getAllByText("$0.84, 1 model unpriced")
        .some((el) => el.getAttribute("title") === "No price for openai ft:gpt-4o:acme"),
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
    expect(within(run).getByText(/1 step, 2 calls/)).toBeInTheDocument();
    expect(within(run).getByText("(1,200 in, 300 out)")).toBeInTheDocument();
    expect(within(run).getAllByText("team=search").length).toBeGreaterThan(0);
    expect(within(run).getByText(RUN)).toBeInTheDocument();
  });

  it("shows the run's scores on its line, the person's own theirs to take back", async () => {
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const scores = within(screen.getByRole("group", { name: "Scores" }));
    expect(scores.getByTitle("helpful, by a member")).toHaveTextContent("helpful 0.80");
    expect(scores.getByTitle("grounded, from your code")).toHaveTextContent("grounded yes");
    expect(scores.getByRole("button", { name: "Take back score helpful" })).toBeInTheDocument();
    expect(scores.queryByRole("button", { name: "Take back score grounded" })).toBeNull();
    expect(scores.getByRole("button", { name: "Score" })).toBeInTheDocument();
  });

  it("takes a tag off the run at its ×, and adds one from the form, each through the action", async () => {
    vi.mocked(setTraceTagsAction).mockResolvedValue({ ok: true, tags: {} });
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const remove = screen.getByRole("button", { name: "Remove tag team" });
    expect(remove.closest("[data-slot=badge]")).toHaveTextContent("team=search");
    fireEvent.click(remove);
    await waitFor(() =>
      expect(setTraceTagsAction).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", RUN, { remove: ["team"] }),
    );
    // Once that change has landed, the next is taken.
    await waitFor(() => expect(remove).not.toHaveAttribute("aria-disabled"));

    fireEvent.click(screen.getByRole("button", { name: "Add tag" }));
    const form = within(await screen.findByRole("dialog", { name: "Add tag" }));
    expect(form.getByRole("button", { name: "Add tag" })).toBeDisabled();
    fireEvent.change(form.getByLabelText("Key"), { target: { value: " repo " } });
    fireEvent.change(form.getByLabelText("Value"), { target: { value: "telmoni" } });
    fireEvent.click(form.getByRole("button", { name: "Add tag" }));
    await waitFor(() =>
      expect(setTraceTagsAction).toHaveBeenLastCalledWith("project_7bQx2mNv9BcK4dLp", RUN, {
        add: { repo: "telmoni" },
      }),
    );
    // Added, the form shuts.
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add tag" })).toBeNull());
  });

  it("links the incidents whose notices showed the run, each at its row of the alerts page", async () => {
    const INCIDENT = "7a1c0e52-0000-4000-8000-000000000002";
    mockIncidents = [{ id: INCIDENT, rule_name: "Failing trips" }];
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(fetchRunIncidents).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", RUN);
    expect(screen.getByRole("link", { name: "incident: Failing trips" })).toHaveAttribute(
      "href",
      `/acme/web/alerts?incident=${INCIDENT}#incident-${INCIDENT}`,
    );
  });

  it("reads the span chosen in the tree: its model, what it was asked with, its tokens, and how it failed", async () => {
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
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
    expect(within(chosen()).getByText("Temperature").nextSibling).toHaveTextContent("0.2");
    expect(within(chosen()).getByText("Seed").nextSibling).toHaveTextContent("7");
    expect(within(chosen()).queryByText("Top p")).toBeNull();
    expect(within(chosen()).getByText("Time to first token").nextSibling).toHaveTextContent("420 ms");
  });

  it("walks the tree from the keyboard, the focus with the span chosen, and shuts a branch and opens it again", async () => {
    // The frame the row waits for, to take the focus, comes at once.
    vi.stubGlobal("requestAnimationFrame", (step: FrameRequestCallback) => {
      step(0);
      return 0;
    });
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
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
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
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
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
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
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("button", { name: "Customize the layout" })).toBeInTheDocument();
    const tree = screen.getByRole("tree", { name: "Spans" });
    expect(within(tree).getAllByRole("treeitem")).toHaveLength(1);
    // The run's own span, chosen, still reads with the whole trace's sums.
    expect(within(chosen()).getByText(/1 step, 2 calls/)).toBeInTheDocument();
    // Customized to its steps, the layout is named as changed. The menu and its
    // sub-menu open from the keyboard, as Radix opens them with no pointer.
    fireEvent.keyDown(screen.getByRole("button", { name: "Customize the layout" }), { key: "Enter" });
    fireEvent.keyDown(await screen.findByRole("menuitem", { name: /^How far a trace opens/ }), {
      key: "ArrowRight",
    });
    fireEvent.click(await screen.findByRole("menuitemradio", { name: "Its steps" }));
    expect(within(screen.getByRole("tree", { name: "Spans" })).getAllByRole("treeitem")).toHaveLength(2);
    expect(screen.getByRole("button", { name: "Customize the layout, changed" })).toBeInTheDocument();
  });

  it("describes each option of the layout's menu by what it means, for a screen reader as for a pointer", async () => {
    mockSetting = { view_id: null };
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    fireEvent.keyDown(screen.getByRole("button", { name: "Customize the layout" }), { key: "Enter" });
    const timeline = await screen.findByRole("menuitemcheckbox", { name: "Timeline" });
    expect(timeline).toHaveAccessibleDescription("Each span's bar on the trace's own line of time.");
  });

  it("opens a saved layout from a link, its pinned spans above the tree, chosen at a press", async () => {
    mockSearch = `view=${LAYOUT_ID}`;
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("button", { name: "Saved layout: Pinned tools" })).toBeInTheDocument();
    const tree = screen.getByRole("tree", { name: "Spans" });
    expect(within(tree).getAllByRole("treeitem")).toHaveLength(2);
    const pin = screen.getByRole("button", { name: /search_docs/ });
    fireEvent.click(pin);
    expect(pin).toHaveAttribute("aria-pressed", "true");
    expect(within(chosen()).getByRole("heading", { name: "search_docs" })).toBeInTheDocument();
  });

  it("shows the run's turn at its head and each span's content in full, the content tab first", async () => {
    const turn = {
      "gen_ai.input.messages": JSON.stringify([
        { role: "user", parts: [{ type: "text", content: "Summarize the report" }] },
      ]),
      "gen_ai.output.messages": JSON.stringify([
        { role: "assistant", parts: [{ type: "text", content: "**Revenue** rose 4%." }], finish_reason: "stop" },
      ]),
    };
    const call = {
      "gen_ai.system_instructions": JSON.stringify([{ type: "text", content: "Answer in one line." }]),
      "gen_ai.output.messages": JSON.stringify([
        {
          role: "assistant",
          parts: [
            { type: "reasoning", content: "The tool knows." },
            { type: "tool_call", id: "c1", name: "weather", arguments: { city: "Paris" } },
          ],
        },
      ]),
      "telmoni.content_cut": JSON.stringify(["gen_ai.input.messages"]),
      "app.ticket": JSON.stringify({ id: 42 }),
    };
    mockTrace = {
      kind: "ok",
      trace: {
        run_id: RUN,
        spans: SPANS,
        truncated: false,
        spend: null,
        content: [
          { run_id: RUN, span_id: "0000000000000001", fields: turn },
          { run_id: RUN, span_id: "0000000000000003", fields: call },
        ],
      },
    };
    mockSetting = { view_id: null };
    const { unmount } = render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getAllByText("Summarize the report").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Revenue")[0]?.tagName).toBe("STRONG");
    expect(within(chosen()).getByRole("tab", { name: "Content" })).toHaveAttribute("aria-selected", "true");
    expect(within(chosen()).getByText("Input")).toBeInTheDocument();
    expect(within(chosen()).getByText(/stopped: stop/)).toBeInTheDocument();
    unmount();

    mockSetting = { view_id: FULL_ID };
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.queryByText("This trace kept no content.")).toBeNull();
    fireEvent.click(within(screen.getByRole("tree", { name: "Spans" })).getByText("chat claude-sonnet-5-5"));
    expect(within(chosen()).getByText("Answer in one line.")).toBeInTheDocument();
    expect(within(chosen()).getByText("Reasoning")).toBeInTheDocument();
    expect(within(chosen()).getByText("weather")).toBeInTheDocument();
    expect(within(chosen()).getByText(/"city": "Paris"/)).toBeInTheDocument();
    expect(within(chosen()).getByText(/Cut to fit an export, and not kept: gen_ai.input.messages/)).toBeInTheDocument();
    expect(within(chosen()).getByText("app.ticket")).toBeInTheDocument();

    fireEvent.click(within(screen.getByRole("tree", { name: "Spans" })).getByText("search_docs"));
    expect(within(chosen()).getByText("This span kept no content.")).toBeInTheDocument();
  });

  it("says in a line when a trace kept no content, and when it could not be read", async () => {
    mockSetting = { view_id: null };
    const { unmount } = render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("This trace kept no content.")).toBeInTheDocument();
    unmount();
    mockTrace = { kind: "ok", trace: { run_id: RUN, spans: SPANS, truncated: false, spend: null, content: null } };
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("This trace's content could not be read. Reload to try again.")).toBeInTheDocument();
  });

  it("calls a run whose own span has not arrived running", async () => {
    mockTrace = {
      kind: "ok",
      trace: { run_id: RUN, spans: SPANS.slice(2, 3), truncated: true, spend: null, content: [] },
    };
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("Running")).toBeInTheDocument();
    expect(within(screen.getByRole("tree")).getByText("chat claude-sonnet-5-5")).toBeInTheDocument();
    expect(screen.getByText(/more spans than the server sends at once/)).toBeInTheDocument();
  });

  it("says when there is no such trace here, and leads back to the list", async () => {
    mockTrace = { kind: "missing" };
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("No trace with this id in this project.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to the traces" })).toHaveAttribute(
      "href",
      "/acme/web/traces",
    );
  });

  it("sends a refused role back to the overview, and says so on an outage", async () => {
    mockTrace = { kind: "forbidden" };
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
      "href",
      "/acme/web",
    );
    mockTrace = { kind: "unavailable" };
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
