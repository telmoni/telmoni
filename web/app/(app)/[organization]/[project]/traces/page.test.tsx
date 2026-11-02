// @vitest-environment jsdom
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({
    title,
    action,
    toolbar,
  }: {
    title: string;
    action?: React.ReactNode;
    toolbar?: React.ReactNode;
  }) => (
    <div data-testid="page-header">
      {title}
      {toolbar}
      {action}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

// The charts measure the box they draw in, which jsdom has none of.
vi.mock("@/components/traces/charts", () => ({
  TraceHistogramStrip: () => <div data-testid="histogram" />,
  TraceCharts: () => <div data-testid="charts" />,
}));

vi.mock("@/app/(app)/[organization]/[project]/traces/actions", () => ({
  saveTraceViewAction: vi.fn(),
  updateTraceViewAction: vi.fn(),
  deleteTraceViewAction: vi.fn(),
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/acme/web/traces",
  useSearchParams: () => new URLSearchParams(),
}));

// The peek is drawn in the console's content area.
vi.mock("@/components/console-shell", () => ({
  useConsoleShell: () => ({ element: document.body, cover: () => () => {} }),
}));

let mockOffset: string | undefined;
vi.mock("next/headers", () => ({
  cookies: async () => ({
    get: (name: string) =>
      name === "telmoni-utc-offset" && mockOffset !== undefined ? { value: mockOffset } : undefined,
  }),
}));

import { TRACE_PREFS_KEY } from "@/lib/trace-prefs";
import { Role } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
let mockTraces: unknown;
let mockFacets: unknown;
let mockHistogram: unknown;
let mockViews: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchTraces = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(
  async () => mockTraces,
);
const fetchTraceFacets = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(
  async () => mockFacets,
);
const fetchTraceHistogram = vi.fn<
  (projectId: string, query: unknown, utcOffset: number) => Promise<unknown>
>(async () => mockHistogram);
const fetchTraceViews = vi.fn<(projectId: string) => Promise<unknown>>(async () => mockViews);
let mockContentMode: unknown = { content_mode: "off", offered: ["off", "on"] };

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchTraces: (projectId: string, query: unknown) => fetchTraces(projectId, query),
  fetchTraceFacets: (projectId: string, query: unknown) => fetchTraceFacets(projectId, query),
  fetchTraceHistogram: (projectId: string, query: unknown, utcOffset: number) =>
    fetchTraceHistogram(projectId, query, utcOffset),
  fetchTraceViews: (projectId: string) => fetchTraceViews(projectId),
  fetchContentMode: async () => mockContentMode,
}));

import TracesPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });
const PROJECT = "project_7bQx2mNv9BcK4dLp";
const RUN_A = "5d4f5c68-0000-4000-8000-000000000001";
const RUN_B = "7a1b2c3d-0000-4000-8000-000000000002";
const USER = "9f86d081884c7d659a2feaa0c55ad015";

const TRACE_A = {
  run_id: RUN_A,
  agent: "nightly-report",
  user_id: USER,
  session_id: "conv-1",
  ended: true,
  status: "ok",
  error_type: "",
  started_at: "2026-10-11T03:00:00Z",
  ended_at: "2026-10-11T03:00:02Z",
  duration_ms: 2000,
  time_to_first_token_ms: null,
  spans: 4,
  steps: 1,
  model_calls: 1,
  tool_calls: 1,
  errors: 1,
  input_tokens: 1200,
  output_tokens: 300,
  cache_read_input_tokens: 100,
  cache_write_input_tokens: 50,
  spend: 0.25,
  models: ["claude-sonnet-5-5"],
  tools: ["search_docs"],
  environments: ["production"],
  releases: ["2026.10.18"],
  tags: ["repo:telmoni", "team:core"],
  scores: [],
};
const TRACE_B = {
  ...TRACE_A,
  run_id: RUN_B,
  agent: "support-bot",
  user_id: "",
  session_id: "",
  ended: false,
  status: "error",
  steps: 0,
  input_tokens: 10,
  output_tokens: 5,
  duration_ms: 500,
  time_to_first_token_ms: null,
  tags: [],
  scores: [],
};

function list(over: Record<string, unknown> = {}) {
  return {
    kind: "ok",
    list: {
      traces: [TRACE_A, TRACE_B],
      days: 7,
      limit: 50,
      page: 1,
      total: 2,
      more: false,
      sort: "started",
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
  mockOffset = undefined;
  mockContext = {
    person: { userId: "user_1", email: "ada@example.test", analyticsOptIn: false },
    organizations: [],
    activeOrganizationId: "org_1",
    memberships: [],
    incomingInvites: [],
    flags: {},
  };
  mockProject = { id: PROJECT, slug: "web", name: "Web", role: Role.Member };
  mockTraces = list();
  mockFacets = {
    kind: "ok",
    facets: {
      days: 7,
      agent: {
        values: [
          { value: "nightly-report", count: 1 },
          { value: "support-bot", count: 1 },
        ],
        more: false,
      },
      user: { values: [{ value: USER, count: 1 }], more: false },
      session: NONE,
      status: {
        values: [
          { value: "ok", count: 1 },
          { value: "error", count: 1 },
        ],
        more: false,
      },
      model: NONE,
      tool: NONE,
      environment: { values: [{ value: "production", count: 1 }], more: false },
      release: NONE,
      tag: NONE,
      prompt: NONE,
    },
  };
  mockHistogram = {
    kind: "ok",
    histogram: { days: 7, bucket_seconds: 10800, utc_offset: 0, bars: [] },
  };
  mockViews = {
    kind: "ok",
    views: [
      {
        id: "0b6f2c1e-6a4f-4d2b-9e3a-5c7d8e9f0a1b",
        name: "Errors",
        query: { status: ["error"] },
        columns: ["started", "status"],
        created_at: "2026-10-11T03:00:00Z",
        updated_at: "2026-10-11T03:00:00Z",
      },
    ],
  };
});

describe("TracesPage", () => {
  // ⚠ The search is offered where the project keeps content, and sent to the
  // server with the filters; a project that keeps none has nothing to search.
  it("offers a search of kept content where the project keeps it, and sends it on", async () => {
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.queryByRole("searchbox", { name: "Search kept content" })).toBeNull();

    mockContentMode = { content_mode: "on", offered: ["off", "on"] };
    try {
      render(
        await TracesPage({ params: PARAMS, searchParams: Promise.resolve({ q: "refund" }) }),
      );
      expect(screen.getByRole("searchbox", { name: "Search kept content" })).toHaveValue("refund");
      expect(fetchTraces).toHaveBeenLastCalledWith(PROJECT, { q: "refund", limit: 50 });
      expect(fetchTraceFacets).toHaveBeenLastCalledWith(PROJECT, { q: "refund" });
      expect(screen.getByRole("link", { name: "Clear search refund" })).toBeInTheDocument();
    } finally {
      mockContentMode = { content_mode: "off", offered: ["off", "on"] };
    }
  });

  it("lists a page of runs in the default columns, each value a filter, and counts them all", async () => {
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Traces");
    expect(screen.getByText("Total 2")).toBeInTheDocument();
    expect(fetchProjectBySlug).toHaveBeenCalledWith("web");
    // A page holds fifty rows unless the URL names another count.
    expect(fetchTraces).toHaveBeenCalledWith(PROJECT, { limit: 50 });
    expect(fetchTraceFacets).toHaveBeenCalledWith(PROJECT, {});
    expect(fetchTraceHistogram).toHaveBeenCalledWith(PROJECT, {}, 0);
    expect(fetchTraceViews).toHaveBeenCalledWith(PROJECT);

    const table = within(screen.getByRole("table"));
    expect(table.getByRole("link", { name: "5d4f5c68" })).toHaveAttribute(
      "href",
      `/acme/web/traces/${RUN_A}`,
    );
    expect(table.getByRole("link", { name: "nightly-report" })).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=nightly-report",
    );
    expect(table.getByRole("link", { name: USER })).toHaveAttribute(
      "href",
      `/acme/web/traces?user=${USER}`,
    );
    expect(table.getByRole("link", { name: "conv-1" })).toHaveAttribute(
      "href",
      "/acme/web/traces?session=conv-1",
    );
    expect(table.getByText("OK")).toBeInTheDocument();
    // A run whose own span has not arrived is running, whatever its spans say.
    expect(table.getByText("Running")).toBeInTheDocument();
    expect(table.queryByText("Error")).toBeNull();
    // A run that recovered from a failed call, and one still running past
    // one, each name it beside the badge.
    expect(table.getAllByText("1 failed span")).toHaveLength(2);
    expect(table.getByText("1,500")).toBeInTheDocument();
    expect(table.getByText("2 s")).toBeInTheDocument();
    expect(table.getByText("500 ms")).toBeInTheDocument();
    // Columns past the first nine are offered, not shown; the count holds the
    // trace's id, which always shows.
    expect(table.queryByRole("columnheader", { name: /Model calls/ })).toBeNull();
    expect(table.getByRole("columnheader", { name: /Scores/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Columns/ })).toHaveTextContent("10/27");
    // The window, the refresh and the columns stand on the title's row.
    const header = within(screen.getByTestId("page-header"));
    expect(header.getByRole("combobox", { name: "Window" })).toHaveTextContent("Last 7 days");
    expect(header.getByRole("button", { name: "Refresh" })).toBeInTheDocument();
    expect(header.getByRole("button", { name: /Columns/ })).toBeInTheDocument();
    expect(screen.getByTestId("histogram")).toBeInTheDocument();
  });

  // The class is metadata, so drawing it files no read; its message is the peek's.
  it("writes a failed run's class under its id, and none for a run still going", async () => {
    mockTraces = list({
      traces: [
        { ...TRACE_A, status: "error", error_type: "RateLimitError" },
        { ...TRACE_B, error_type: "RateLimitError" },
      ],
    });
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const table = within(screen.getByRole("table"));
    expect(table.getByRole("link", { name: "5d4f5c68" }).closest("td")).toHaveTextContent(
      "RateLimitError",
    );
    expect(table.getByRole("link", { name: "7a1b2c3d" }).closest("td")).not.toHaveTextContent(
      "RateLimitError",
    );
  });

  // In Chrome a scroll into view moves where the page's first Tab starts, past the skip link.
  it("chooses no facet's value as its list mounts, so none is scrolled into view", async () => {
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(document.querySelectorAll("[cmdk-item]").length).toBeGreaterThan(0);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(document.querySelector('[cmdk-item][aria-selected="true"]')).toBeNull();
    expect(Element.prototype.scrollIntoView).not.toHaveBeenCalled();
  });

  it("reads each filter's values, the sort, the window and the page, and hands them to the server", async () => {
    render(
      await TracesPage({
        params: PARAMS,
        searchParams: Promise.resolve({
          agent: ["nightly-report", "support-bot"],
          user: USER,
          sort: "tokens",
          days: "30",
          status: "bogus",
          page: "2",
          limit: "25",
        }),
      }),
    );
    const query = {
      agent: ["nightly-report", "support-bot"],
      user: [USER],
      sort: "tokens",
      days: 30,
      limit: 25,
      page: 2,
    };
    expect(fetchTraces).toHaveBeenCalledWith(PROJECT, query);
    expect(fetchTraceFacets).toHaveBeenCalledWith(PROJECT, query);
    expect(fetchTraceHistogram).toHaveBeenCalledWith(PROJECT, query, 0);

    // Each value in force is a chip that clears it, from the first page again.
    expect(screen.getByRole("link", { name: "Clear agent support-bot" })).toHaveAttribute(
      "href",
      `/acme/web/traces?agent=nightly-report&user=${USER}&sort=tokens&days=30&limit=25`,
    );
    expect(screen.getByRole("link", { name: `Clear user ${USER}` })).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=nightly-report&agent=support-bot&sort=tokens&days=30&limit=25",
    );
    // The user in force alone is no longer a link in its column.
    const table = within(screen.getByRole("table"));
    expect(table.queryByRole("link", { name: USER })).toBeNull();
    // Tokens sorts most first; a second click flips it, from the first page.
    expect(table.getByRole("columnheader", { name: /Tokens/ })).toHaveAttribute("aria-sort", "descending");
    expect(table.getByRole("link", { name: /Tokens/ })).toHaveAttribute(
      "href",
      `/acme/web/traces?agent=nightly-report&agent=support-bot&user=${USER}&sort=tokens&days=30&limit=25&dir=asc`,
    );
    expect(table.getByRole("link", { name: "Started" })).toHaveAttribute(
      "href",
      `/acme/web/traces?agent=nightly-report&agent=support-bot&user=${USER}&sort=started&days=30&limit=25&dir=desc`,
    );
    expect(screen.getByRole("combobox", { name: "Window" })).toHaveTextContent("Last 30 days");
  });

  it("counts the pages and offers the way to each, keeping the rows a page holds", async () => {
    mockTraces = list({ total: 120, page: 2 });
    render(
      await TracesPage({ params: PARAMS, searchParams: Promise.resolve({ page: "2", agent: "zeta" }) }),
    );
    expect(screen.getByText("Total 120")).toBeInTheDocument();
    expect(screen.getByText("Page 2 of 3")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Next page" })).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=zeta&page=3",
    );
    expect(screen.getByRole("link", { name: "First page" })).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=zeta",
    );
    expect(screen.getByRole("link", { name: "Last page" })).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=zeta&page=3",
    );
    expect(screen.getByRole("combobox", { name: "Rows per page" })).toHaveTextContent("50");
  });

  it("says when a page is past the last, and leads to the last", async () => {
    mockTraces = list({ traces: [], total: 5, page: 4 });
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({ page: "4" }) }));
    expect(screen.getByText("This page is past the last one.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Go to the last page" })).toHaveAttribute(
      "href",
      "/acme/web/traces",
    );
  });

  it("tells an empty project how to send its first run, and a filtered one to widen", async () => {
    mockTraces = list({ traces: [], total: 0 });
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "SDK" })).toHaveAttribute("href", "/docs/api/sdks");
    expect(screen.getByRole("link", { name: "coding-agent plugin" })).toHaveAttribute(
      "href",
      "/docs/api/coding-agents",
    );
    expect(screen.queryByTestId("histogram")).toBeNull();

    render(
      await TracesPage({
        params: PARAMS,
        searchParams: Promise.resolve({ agent: "nobody", sort: "tokens" }),
      }),
    );
    expect(screen.getByText("No traces match these filters.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "clear the filters" })).toHaveAttribute(
      "href",
      "/acme/web/traces?sort=tokens",
    );
  });

  it("says an empty project has nothing to chart, rather than drawing empty charts", async () => {
    mockTraces = list({ traces: [], total: 0 });
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({ display: "chart" }) }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.queryByTestId("charts")).toBeNull();
  });

  it("shows the charts in the table's place, and draws the bars on the reader's clock", async () => {
    mockOffset = "330";
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({ display: "chart" }) }));
    expect(fetchTraceHistogram).toHaveBeenCalledWith(PROJECT, {}, 330);
    expect(screen.getByTestId("charts")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.getByRole("tab", { name: /Chart/ })).toHaveAttribute("aria-selected", "true");
  });

  it("shows each run's tags as chips in the Tags column, each narrowing the list to it", async () => {
    window.localStorage.setItem(TRACE_PREFS_KEY, JSON.stringify({ columns: ["agent", "tags"] }));
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({ tag: "team:core" }) }));
    const table = within(screen.getByRole("table"));
    expect(table.getByRole("columnheader", { name: /Tags/ })).toBeInTheDocument();
    const row = within(table.getByRole("link", { name: "5d4f5c68" }).closest("tr")!);
    expect(row.getByRole("link", { name: "repo:telmoni" })).toHaveAttribute(
      "href",
      "/acme/web/traces?tag=repo%3Atelmoni",
    );
    // The tag the list is narrowed to alone is a chip, no longer a link.
    expect(row.queryByRole("link", { name: "team:core" })).toBeNull();
    expect(row.getByText("team:core").closest("[data-slot=badge]")).toBeInTheDocument();
    // A run with no tags shows a dash.
    expect(within(table.getByRole("link", { name: "7a1b2c3d" }).closest("tr")!).getByText("—")).toBeInTheDocument();
  });

  it("sorts by a tag's key picked from the Tags heading, the keys those the window's tag facet names", async () => {
    window.localStorage.setItem(TRACE_PREFS_KEY, JSON.stringify({ columns: ["agent", "tags"] }));
    (mockFacets as { facets: Record<string, unknown> }).facets.tag = {
      values: [
        { value: "team:core", count: 2 },
        { value: "repo:telmoni", count: 1 },
        { value: "team:search", count: 1 },
      ],
      more: false,
    };
    const { unmount } = render(
      await TracesPage({ params: PARAMS, searchParams: Promise.resolve({ agent: "zeta" }) }),
    );
    const heading = screen.getByRole("columnheader", { name: /Tags/ });
    expect(heading).toHaveAttribute("aria-sort", "none");
    fireEvent.keyDown(within(heading).getByRole("button", { name: "Tags" }), { key: "Enter" });
    expect(await screen.findByText("Sort by tag")).toBeInTheDocument();
    expect(
      screen.getAllByRole("menuitem").map((item) => [item.textContent, item.getAttribute("href")]),
    ).toEqual([
      ["repo", "/acme/web/traces?agent=zeta&sort=tag%3Arepo&dir=asc"],
      ["team", "/acme/web/traces?agent=zeta&sort=tag%3Ateam&dir=asc"],
    ]);
    unmount();

    // Sorted by one, the heading names it, offered though the window's tags
    // no longer hold it, and picking it again turns it round.
    (mockFacets as { facets: Record<string, unknown> }).facets.tag = {
      values: [{ value: "repo:telmoni", count: 1 }],
      more: false,
    };
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({ sort: "tag:team" }) }));
    expect(fetchTraces).toHaveBeenLastCalledWith(PROJECT, { sort: "tag:team", limit: 50 });
    const sorted = screen.getByRole("columnheader", { name: /Tags/ });
    expect(sorted).toHaveAttribute("aria-sort", "ascending");
    expect(within(sorted).getByText("team")).toBeInTheDocument();
    fireEvent.keyDown(within(sorted).getByRole("button", { name: "Tags, sorted by team" }), { key: "Enter" });
    const team = await screen.findByRole("menuitem", { name: "team" });
    expect(team).toHaveAttribute("href", "/acme/web/traces?sort=tag%3Ateam&dir=desc");
    expect(team).toHaveAttribute("aria-current", "true");
    expect(screen.getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["repo", "team"]);
  });

  it("describes each column in its menu by what it means, for a screen reader as for a pointer", async () => {
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    fireEvent.keyDown(screen.getByRole("button", { name: /^Columns/ }), { key: "Enter" });
    const trace = await screen.findByRole("menuitemcheckbox", { name: "Trace ID" });
    expect(trace).toHaveAccessibleDescription("The run's id, which opens its trace. Always shown.");
  });

  it("names the project's saved views, and the one the page was opened from", async () => {
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("button", { name: "Saved views" })).toHaveTextContent("1");

    render(
      await TracesPage({
        params: PARAMS,
        searchParams: Promise.resolve({ status: "error", view: "0b6f2c1e-6a4f-4d2b-9e3a-5c7d8e9f0a1b" }),
      }),
    );
    expect(screen.getByRole("button", { name: "Saved view: Errors" })).toBeInTheDocument();
  });

  it("keeps the list when the facets, the histogram or the views cannot be read", async () => {
    mockFacets = { kind: "unavailable" };
    mockHistogram = { kind: "unavailable" };
    mockViews = { kind: "unavailable" };
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("table")).toBeInTheDocument();
    expect(screen.getByText(/The filters.*could not be read/)).toBeInTheDocument();
    expect(screen.queryByTestId("histogram")).toBeNull();
  });

  it("sends a refused role back to the project's overview, and says so on an outage", async () => {
    mockTraces = { kind: "forbidden" };
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
      "href",
      "/acme/web",
    );

    mockTraces = { kind: "unavailable" };
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });

  it("renders ServiceUnavailable when context fails", async () => {
    mockContext = null;
    render(await TracesPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
