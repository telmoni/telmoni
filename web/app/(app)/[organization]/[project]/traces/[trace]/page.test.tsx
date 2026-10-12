// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
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
let mockTrace: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchTrace = vi.fn<(projectId: string, runId: string) => Promise<unknown>>(
  async () => mockTrace,
);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchTrace: (projectId: string, runId: string) => fetchTrace(projectId, runId),
}));

import TracePage from "./page";

const RUN = "5d4f5c68-0000-4000-8000-000000000001";
const PARAMS = Promise.resolve({ organization: "acme", project: "web", trace: RUN });

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
  it("names the run, sums it, and lists its steps with their calls", async () => {
    render(await TracePage({ params: PARAMS }));
    expect(fetchTrace).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", RUN);
    const header = screen.getByTestId("page-header");
    expect(header).toHaveTextContent("nightly-report");
    expect(header).toHaveAttribute("data-back", "/acme/web/traces");

    expect(screen.getByText("2 s")).toBeInTheDocument();
    // The run's tokens, and the one call's: the same number twice.
    expect(screen.getAllByText("1,500")).toHaveLength(2);
    expect(screen.getByText("(1,200 in, 300 out)")).toBeInTheDocument();
    expect(screen.getByText("40 steps at most")).toBeInTheDocument();
    expect(screen.getByText("team=search")).toBeInTheDocument();
    expect(screen.getByText(RUN)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "9f86d081884c7d659a2feaa0c55ad015" })).toHaveAttribute(
      "href",
      "/acme/web/traces?user=9f86d081884c7d659a2feaa0c55ad015",
    );
    // The session leads to its own page, the user to their traces.
    expect(screen.getByRole("link", { name: "conv-1" })).toHaveAttribute(
      "href",
      "/acme/web/sessions/conv-1",
    );

    // The steps view: the step, then its two calls under it.
    expect(screen.getByText("plan the reply")).toBeInTheDocument();
    expect(screen.getByText("chat claude-sonnet-5-5")).toBeInTheDocument();
    expect(screen.getByText("claude-sonnet-5-5")).toBeInTheDocument();
    expect(screen.getByText("Error: TimeoutError")).toBeInTheDocument();
    expect(screen.getByText("1.5 s")).toBeInTheDocument();
    expect(screen.getByText("200 ms")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Timeline" })).toHaveAttribute(
      "href",
      `/acme/web/traces/${RUN}?view=timeline`,
    );
  });

  it("draws the timeline one view away, every span a bar", async () => {
    render(
      await TracePage({ params: PARAMS, searchParams: Promise.resolve({ view: "timeline" }) }),
    );
    expect(screen.getByRole("heading", { name: "Timeline" })).toBeInTheDocument();
    expect(screen.getAllByRole("listitem")).toHaveLength(4);
    expect(screen.getByRole("link", { name: "Steps" })).toHaveAttribute(
      "href",
      `/acme/web/traces/${RUN}`,
    );
  });

  it("calls a run whose own span has not arrived running", async () => {
    mockTrace = { kind: "ok", trace: { run_id: RUN, spans: SPANS.slice(2, 3), truncated: true } };
    render(await TracePage({ params: PARAMS }));
    expect(screen.getByText("Running")).toBeInTheDocument();
    expect(screen.getByText("Under the run")).toBeInTheDocument();
    expect(screen.getByText(/more spans than the page shows/)).toBeInTheDocument();
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
