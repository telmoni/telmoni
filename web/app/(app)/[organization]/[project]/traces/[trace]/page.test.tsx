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

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/acme/web/traces/5d4f5c68000040008000000000000001",
  useSearchParams: () => new URLSearchParams(),
}));

vi.mock("@/app/(app)/[organization]/[project]/traces/actions", () => ({}));

import { testSpan } from "@/lib/span-fixture";
import { Role, SpanFault, SpanKind } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
let mockTrace: unknown;
let mockIncidents: unknown[];
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchTrace = vi.fn<(projectId: string, traceId: string, content: boolean) => Promise<unknown>>(
  async () => mockTrace,
);
const fetchTraceIncidents = vi.fn<(projectId: string, traceId: string) => Promise<unknown[]>>(
  async () => mockIncidents,
);

const SCORES = vi.hoisted(() => [
  {
    score_id: "5d4f5c68-0000-4000-8000-0000000000a1",
    target: "trace",
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
    target: "trace",
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
  fetchTrace: (projectId: string, traceId: string, content: boolean) => fetchTrace(projectId, traceId, content),
  fetchTraceIncidents: (projectId: string, traceId: string) => fetchTraceIncidents(projectId, traceId),
}));

import TracePage from "./page";

const TRACE_ID = "5d4f5c68000040008000000000000001";
const PARAMS = Promise.resolve({ organization: "acme", project: "web", trace: TRACE_ID });
const span = (over: Record<string, unknown>) =>
  testSpan({
    trace_id: TRACE_ID,
    span_id: "0000000000000001",
    kind: SpanKind.Agent,
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
  window.localStorage.clear();
  mockContext = {
    person: { userId: "user_1", email: "ada@example.test" },
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
      trace_id: TRACE_ID,
      spans: SPANS,
      truncated: false,
      spend: { spent: 0.84, unpriced: [{ provider: "openai", model: "ft:gpt-4o:acme" }] },
      content: [],
    },
  };
  mockIncidents = [];
});

describe("TracePage", () => {
  // Four days after the trace, inside the week the user's and the session's
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

  it("names the trace, sums it on one line, and opens on the agent's own span", async () => {
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(fetchTrace).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", TRACE_ID, true);
    const header = screen.getByTestId("page-header");
    expect(header).toHaveTextContent("nightly-report");
    expect(header).toHaveAttribute("data-back", "/acme/web/traces");

    // The line under the name: how long, the model calls' tokens, and the
    // session and the user it carried, each leading to its page.
    expect(screen.getAllByText("2 s").length).toBeGreaterThan(0);
    expect(
      screen.getAllByText("1,500 tokens").some((el) => el.getAttribute("title") === "1,200 in, 300 out"),
    ).toBe(true);
    // ⚠ What the trace spent, as its cost alert's notice gives it, and the models left out.
    expect(
      screen
        .getAllByText("$0.84, 1 model unpriced")
        .some((el) => el.getAttribute("title") === "No price for openai ft:gpt-4o:acme"),
    ).toBe(true);
    expect(screen.getByRole("link", { name: "session" })).toHaveAttribute(
      "href",
      "/acme/web/traces?session=conv-1&sort=started&dir=asc",
    );
    expect(screen.getByRole("link", { name: "user 9f86d081" })).toHaveAttribute(
      "href",
      "/acme/web/traces?user=9f86d081884c7d659a2feaa0c55ad015",
    );

    // The spans as they ran: the agent, its step, and the step's two calls.
    const tree = screen.getByRole("tree", { name: "Spans" });
    const rows = within(tree).getAllByRole("treeitem");
    expect(rows.map((row) => row.getAttribute("aria-level"))).toEqual(["1", "2", "3", "3"]);
    expect(rows[0]).toHaveAttribute("aria-selected", "true");
    expect(within(tree).getByText("TimeoutError")).toBeInTheDocument();

    // The agent's own span, read with the whole trace's sums.
    const agent = chosen();
    expect(within(agent).getByRole("heading", { name: "nightly-report" })).toBeInTheDocument();
    expect(within(agent).getByText(/1 step, 2 calls/)).toBeInTheDocument();
    expect(within(agent).getByText("(1,200 in, 300 out)")).toBeInTheDocument();
    expect(within(agent).getAllByText("team=search").length).toBeGreaterThan(0);
    expect(within(agent).getAllByText(TRACE_ID).length).toBeGreaterThan(0);
  });

  it("shows the trace's scores on its line, the person's own theirs to take back", async () => {
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const scores = within(screen.getByRole("group", { name: "Scores" }));
    expect(scores.getByTitle("helpful, by a member")).toHaveTextContent("helpful 0.80");
    expect(scores.getByTitle("grounded, from your code")).toHaveTextContent("grounded yes");
    expect(scores.getByRole("button", { name: "Take back score helpful" })).toBeInTheDocument();
    expect(scores.queryByRole("button", { name: "Take back score grounded" })).toBeNull();
    expect(scores.getByRole("button", { name: "Score" })).toBeInTheDocument();
  });

  it("says on the trace's line whether the project keeps content", async () => {
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTitle(/This project keeps prompts, replies and tool payloads/)).toHaveTextContent("Content on");
  });

  it("links each of the trace's tags to the traces that carry it", async () => {
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const tag = screen.getByRole("link", { name: "team:search" });
    expect(tag.getAttribute("href")).toContain("tag=team%3Asearch");
  });

  it("links the incidents whose notices showed the trace, each at its row of the alerts page", async () => {
    const INCIDENT = "7a1c0e52-0000-4000-8000-000000000002";
    mockIncidents = [{ id: INCIDENT, rule_name: "Failing trips" }];
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(fetchTraceIncidents).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", TRACE_ID);
    expect(screen.getByRole("link", { name: "incident: Failing trips" })).toHaveAttribute(
      "href",
      `/acme/web/alerts?incident=${INCIDENT}#incident-${INCIDENT}`,
    );
  });

  it("reads the span chosen in the tree: its model, its tokens, and how it failed", async () => {
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const tree = screen.getByRole("tree", { name: "Spans" });

    fireEvent.click(within(tree).getByText("search_docs"));
    expect(within(chosen()).getByRole("heading", { name: "search_docs" })).toBeInTheDocument();
    // A red square, its error's class beside it, the whole of it its title.
    const failed = within(chosen()).getAllByTitle("Error: TimeoutError");
    expect(failed.length).toBeGreaterThan(0);
    expect(failed[0]).toHaveTextContent("Error: TimeoutError");
    expect(within(chosen()).getAllByText("200 ms").length).toBeGreaterThan(0);

    fireEvent.click(within(tree).getByText("chat claude-sonnet-5-5"));
    expect(within(chosen()).getAllByRole("link", { name: "claude-sonnet-5-5" })[0]).toHaveAttribute(
      "href",
      "/acme/web/traces?model=claude-sonnet-5-5",
    );
    expect(within(chosen()).getByText("1,200")).toBeInTheDocument();
    expect(within(chosen()).getByText("anthropic")).toBeInTheDocument();
    expect(within(chosen()).getByText("Time to first token").nextSibling).toHaveTextContent("420 ms");
  });

  it("shows a refused model call's fault without changing its successful status", async () => {
    mockTrace = { kind: "ok", trace: { trace_id: TRACE_ID,
      spans: SPANS.map((span) => span.kind === SpanKind.ModelCall ? { ...span, fault: SpanFault.Refused } : span),
      truncated: false, spend: null, content: [] } };
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    fireEvent.click(within(screen.getByRole("tree", { name: "Spans" })).getByText("chat claude-sonnet-5-5"));
    expect(within(chosen()).getByText("Fault").nextSibling).toHaveTextContent("Refused");
    expect(within(chosen()).getAllByText("OK").length).toBeGreaterThan(0);
  });

  it("draws a retrieval by its source with its count, beside the trace's other calls", async () => {
    const found = span({
      span_id: "0000000000000005",
      parent_span_id: "0000000000000002",
      kind: "retrieval",
      name: "docs-index",
      attributes: {
        "gen_ai.data_source.id": "docs-index",
        "gen_ai.retrieval.top_k": "5",
        "telmoni.retrieval.returned": "3",
      },
      started_at: "2026-10-11T03:00:01.820Z",
      ended_at: "2026-10-11T03:00:01.900Z",
    });
    // A memory search that found nothing, and asked for no number.
    const recalled = span({
      span_id: "0000000000000006",
      parent_span_id: "0000000000000002",
      kind: "retrieval",
      name: "preferences",
      attributes: { "gen_ai.memory.store.id": "preferences", "gen_ai.memory.record.count": "0" },
      started_at: "2026-10-11T03:00:01.905Z",
      ended_at: "2026-10-11T03:00:01.909Z",
    });
    mockTrace = {
      kind: "ok",
      trace: { trace_id: TRACE_ID, spans: [...SPANS, found, recalled], truncated: false, spend: null, content: [] },
    };
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const tree = screen.getByRole("tree", { name: "Spans" });
    const row = (name: string) =>
      within(tree)
        .getAllByRole("treeitem")
        .find((item) => item.textContent?.includes(name))!;
    expect(within(row("docs-index")).getByText("3 of 5")).toBeInTheDocument();
    // ⚠ Nothing found is a count of 0, never a count left out.
    expect(within(row("preferences")).getByText("0")).toBeInTheDocument();

    expect(within(chosen()).getByText(/1 step, 4 calls/)).toBeInTheDocument();
    expect(within(chosen()).getByText("1 to a model, 1 to a tool, 2 retrievals")).toBeInTheDocument();

    const field = (label: string) => within(chosen()).queryByText(label)?.nextSibling;
    fireEvent.click(within(tree).getByText("docs-index"));
    expect(field("Source")).toHaveTextContent("docs-index");
    expect(field("Asked for")).toHaveTextContent("5");
    expect(field("Returned")).toHaveTextContent("3");
    fireEvent.click(within(tree).getByText("preferences"));
    expect(field("Source")).toHaveTextContent("preferences");
    expect(field("Returned")).toHaveTextContent("0");
    expect(within(chosen()).queryByText("Asked for")).toBeNull();
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

  it("shows the trace's turn at its head and each span's content in full, the content tab first", async () => {
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
        trace_id: TRACE_ID,
        spans: SPANS,
        truncated: false,
        spend: null,
        content: [
          { trace_id: TRACE_ID, span_id: "0000000000000001", fields: turn },
          { trace_id: TRACE_ID, span_id: "0000000000000003", fields: call },
        ],
      },
    };
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getAllByText("Summarize the report").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Revenue")[0]?.tagName).toBe("STRONG");
    expect(within(chosen()).getByRole("tab", { name: "Content" })).toHaveAttribute("aria-selected", "true");
    expect(within(chosen()).getByText("Input")).toBeInTheDocument();
    expect(within(chosen()).getByText(/stopped: stop/)).toBeInTheDocument();
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

  it("says what the scrubbers replaced in a span's content, and what they dropped", async () => {
    const scrubbed = SPANS.map((one) =>
      one.span_id === "0000000000000003"
        ? { ...one, attributes: { "telmoni.scrubbed": '{"dropped:gen_ai.tool.call.result":1,"pii:email":2,"secret:aws_key":1}' } }
        : one,
    );
    mockTrace = {
      kind: "ok",
      trace: {
        trace_id: TRACE_ID,
        spans: scrubbed,
        truncated: false,
        spend: null,
        content: [
          {
            trace_id: TRACE_ID,
            span_id: "0000000000000003",
            fields: {
              "gen_ai.output.messages": JSON.stringify([
                { role: "assistant", parts: [{ type: "text", content: "Key [secret: aws_key] for [pii: email]" }] },
              ]),
            },
          },
        ],
      },
    };
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    fireEvent.click(within(screen.getByRole("tree", { name: "Spans" })).getByText("chat claude-sonnet-5-5"));
    expect(
      within(chosen()).getByText("Scrubbed by the SDK or at the door: 2 email addresses, 1 AWS key."),
    ).toBeInTheDocument();
    expect(
      within(chosen()).getByText("Dropped, its scrubber failing on it: gen_ai.tool.call.result."),
    ).toBeInTheDocument();
    expect(within(chosen()).getByText("Key [secret: aws_key] for [pii: email]")).toBeInTheDocument();
  });

  it("says in a line when a trace kept no content, and when it could not be read", async () => {
    const { unmount } = render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("This trace kept no content.")).toBeInTheDocument();
    unmount();
    mockTrace = { kind: "ok", trace: { trace_id: TRACE_ID, spans: SPANS, truncated: false, spend: null, content: null } };
    render(await TracePage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("This trace's content could not be read. Reload to try again.")).toBeInTheDocument();
  });

  it("calls a trace whose agent's span has not arrived running", async () => {
    mockTrace = {
      kind: "ok",
      trace: { trace_id: TRACE_ID, spans: SPANS.slice(2, 3), truncated: true, spend: null, content: [] },
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
