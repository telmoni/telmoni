// @vitest-environment jsdom
import { render, screen, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({
    title,
    count,
    action,
  }: {
    title: string;
    count?: number;
    action?: React.ReactNode;
  }) => (
    <div data-testid="page-header">
      {title}
      {count !== undefined && <span data-testid="count">{count}</span>}
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

let mockOffset: string | undefined;
vi.mock("next/headers", () => ({
  cookies: async () => ({
    get: (name: string) =>
      name === "telmoni-utc-offset" && mockOffset !== undefined ? { value: mockOffset } : undefined,
  }),
}));

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

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchTraces: (projectId: string, query: unknown) => fetchTraces(projectId, query),
  fetchTraceFacets: (projectId: string, query: unknown) => fetchTraceFacets(projectId, query),
  fetchTraceHistogram: (projectId: string, query: unknown, utcOffset: number) =>
    fetchTraceHistogram(projectId, query, utcOffset),
  fetchTraceViews: (projectId: string) => fetchTraceViews(projectId),
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
  spans: 4,
  steps: 1,
  model_calls: 1,
  tool_calls: 1,
  errors: 1,
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
  agent: "support-bot",
  user_id: "",
  session_id: "",
  ended: false,
  status: "error",
  steps: 0,
  input_tokens: 10,
  output_tokens: 5,
  duration_ms: 500,
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
          { value: "nightly-report", runs: 1 },
          { value: "support-bot", runs: 1 },
        ],
        more: false,
      },
      user: { values: [{ value: USER, runs: 1 }], more: false },
      session: NONE,
      status: {
        values: [
          { value: "ok", runs: 1 },
          { value: "error", runs: 1 },
        ],
        more: false,
      },
      model: NONE,
      tool: NONE,
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
  it("lists a page of runs in the default columns, each value a filter, and counts them all", async () => {
    render(await TracesPage({ params: PARAMS }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Traces");
    expect(screen.getByTestId("count")).toHaveTextContent("2");
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
    expect(table.getByText("1,500")).toBeInTheDocument();
    expect(table.getByText("2 s")).toBeInTheDocument();
    expect(table.getByText("500 ms")).toBeInTheDocument();
    // Columns past the first eight are offered, not shown; the count, as
    // Langfuse's, holds the trace's id, which always shows.
    expect(table.queryByRole("columnheader", { name: /Model calls/ })).toBeNull();
    expect(screen.getByRole("button", { name: /Columns/ })).toHaveTextContent("9/21");
    // The window and the refresh stand on the title's row.
    const header = within(screen.getByTestId("page-header"));
    expect(header.getByRole("combobox", { name: "Window" })).toHaveTextContent("Last 7 days");
    expect(header.getByRole("button", { name: "Refresh" })).toBeInTheDocument();
    expect(screen.getByTestId("histogram")).toBeInTheDocument();
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
    expect(screen.getByTestId("count")).toHaveTextContent("120");
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
    render(await TracesPage({ params: PARAMS }));
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

  it("names the project's saved views, and the one the page was opened from", async () => {
    render(await TracesPage({ params: PARAMS }));
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
    render(await TracesPage({ params: PARAMS }));
    expect(screen.getByRole("table")).toBeInTheDocument();
    expect(screen.getByText(/The filters.*could not be read/)).toBeInTheDocument();
    expect(screen.queryByTestId("histogram")).toBeNull();
  });

  it("sends a refused role back to the project's overview, and says so on an outage", async () => {
    mockTraces = { kind: "forbidden" };
    render(await TracesPage({ params: PARAMS }));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
      "href",
      "/acme/web",
    );

    mockTraces = { kind: "unavailable" };
    render(await TracesPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });

  it("renders ServiceUnavailable when context fails", async () => {
    mockContext = null;
    render(await TracesPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
