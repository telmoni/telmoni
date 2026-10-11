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
let mockTraces: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchTraces = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(
  async () => mockTraces,
);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchTraces: (projectId: string, query: unknown) => fetchTraces(projectId, query),
}));

import TracesPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });
const RUN_A = "5d4f5c68-0000-4000-8000-000000000001";
const RUN_B = "7a1b2c3d-0000-4000-8000-000000000002";

const TRACE_A = {
  run_id: RUN_A,
  agent: "nightly-report",
  user_id: "9f86d081884c7d659a2feaa0c55ad015",
  conversation_id: "conv-1",
  ended: true,
  status: "ok",
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
  conversation_id: "",
  ended: false,
  status: "error",
  steps: 0,
  input_tokens: 10,
  output_tokens: 5,
  duration_ms: 500,
};

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
  mockTraces = {
    kind: "ok",
    list: { traces: [TRACE_A, TRACE_B], days: 7, limit: 100, more: false, sort: "started", dir: "desc" },
  };
});

describe("TracesPage", () => {
  it("lists each run with its agent, user, conversation, status, steps, tokens and duration", async () => {
    render(await TracesPage({ params: PARAMS }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Traces");
    expect(screen.getByTestId("count")).toHaveTextContent("2");
    expect(fetchProjectBySlug).toHaveBeenCalledWith("web");
    expect(fetchTraces).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", {});

    expect(screen.getByRole("link", { name: "5d4f5c68" })).toHaveAttribute(
      "href",
      `/acme/web/traces/${RUN_A}`,
    );
    expect(screen.getByRole("link", { name: "nightly-report" })).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=nightly-report",
    );
    expect(screen.getByRole("link", { name: "9f86d081884c7d659a2feaa0c55ad015" })).toHaveAttribute(
      "href",
      "/acme/web/traces?user=9f86d081884c7d659a2feaa0c55ad015",
    );
    expect(screen.getByRole("link", { name: "conv-1" })).toHaveAttribute(
      "href",
      "/acme/web/traces?conversation=conv-1",
    );
    // The filter form offers every status as an option; the badges are the rest.
    const badges = (text: string) =>
      screen.getAllByText(text).filter((el) => el.tagName !== "OPTION");
    expect(badges("OK")).toHaveLength(1);
    // A run whose own span has not arrived is running, whatever its spans say.
    expect(screen.getByText("Running")).toBeInTheDocument();
    expect(screen.queryAllByText("Error").filter((el) => el.tagName !== "OPTION")).toHaveLength(0);
    expect(screen.getByText("1,500")).toBeInTheDocument();
    expect(screen.getByText("2 s")).toBeInTheDocument();
    expect(screen.getByText("500 ms")).toBeInTheDocument();
  });

  it("reads the URL's filters and sort, hands them to the server, and offers each heading as a sort", async () => {
    render(
      await TracesPage({
        params: PARAMS,
        searchParams: Promise.resolve({ user: "9f86d081884c7d659a2feaa0c55ad015", sort: "tokens", days: "30", status: "bogus" }),
      }),
    );
    expect(fetchTraces).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", {
      user: "9f86d081884c7d659a2feaa0c55ad015",
      sort: "tokens",
      days: 30,
    });
    // The user in force is no longer a link, and can be cleared.
    expect(screen.queryByRole("link", { name: "9f86d081884c7d659a2feaa0c55ad015" })).toBeNull();
    expect(screen.getByRole("link", { name: "Clear the user filter" })).toHaveAttribute(
      "href",
      "/acme/web/traces?sort=tokens&days=30",
    );
    // Tokens sorts most first; a second click flips it, and another column starts its own way.
    expect(screen.getByRole("columnheader", { name: /Tokens/ })).toHaveAttribute("aria-sort", "descending");
    expect(screen.getByRole("link", { name: /Tokens/ })).toHaveAttribute(
      "href",
      "/acme/web/traces?user=9f86d081884c7d659a2feaa0c55ad015&sort=tokens&days=30&dir=asc",
    );
    expect(screen.getByRole("link", { name: "Conversation" })).toHaveAttribute(
      "href",
      "/acme/web/traces?user=9f86d081884c7d659a2feaa0c55ad015&sort=conversation&days=30&dir=asc",
    );
    expect(screen.getByRole("link", { name: "Started" })).toHaveAttribute(
      "href",
      "/acme/web/traces?user=9f86d081884c7d659a2feaa0c55ad015&sort=started&days=30&dir=desc",
    );
  });

  it("says when the window holds more, and offers the larger page", async () => {
    mockTraces = {
      kind: "ok",
      list: { traces: [TRACE_A], days: 7, limit: 100, more: true, sort: "started", dir: "desc" },
    };
    render(await TracesPage({ params: PARAMS }));
    expect(screen.queryByTestId("count")).toBeNull();
    expect(screen.getByRole("link", { name: "Show up to 500" })).toHaveAttribute(
      "href",
      "/acme/web/traces?limit=500",
    );
  });

  it("tells an empty project how to send its first run, and a filtered one to widen", async () => {
    mockTraces = {
      kind: "ok",
      list: { traces: [], days: 7, limit: 100, more: false, sort: "started", dir: "desc" },
    };
    render(await TracesPage({ params: PARAMS }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "SDK" })).toHaveAttribute("href", "/docs/api/sdks");

    render(
      await TracesPage({ params: PARAMS, searchParams: Promise.resolve({ agent: "nobody" }) }),
    );
    expect(screen.getByText("No traces match these filters.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "clear the filters" })).toHaveAttribute(
      "href",
      "/acme/web/traces",
    );
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
