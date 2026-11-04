// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => <div data-testid="page-header">{title}</div>,
}));
vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));
vi.mock("@/components/access-denied", () => ({
  AccessDenied: () => <div data-testid="denied">denied</div>,
}));
vi.mock("@/components/traces/compare-runs", () => ({
  CompareRuns: ({ a, b, keepsContent }: { a: { run_id: string }; b: { run_id: string }; keepsContent: boolean }) => (
    <div data-testid="compared" data-content={String(keepsContent)}>
      {a.run_id} {b.run_id}
    </div>
  ),
}));

const fetchTrace = vi.fn<(projectId: string, runId: string, content: boolean) => Promise<unknown>>();
let mockMode: unknown;
vi.mock("@/lib/server/project-page", () => ({
  projectPage: async () => ({ kind: "ok", gate: {}, project: { id: "project_7bQx2mNv9BcK4dLp" } }),
}));
vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  fetchTrace: (projectId: string, runId: string, content: boolean) => fetchTrace(projectId, runId, content),
}));
vi.mock("@/lib/server/entities/content-mode", () => ({ fetchContentMode: async () => mockMode }));

import ComparePage from "./page";

const A = "5d4f5c68-0000-4000-8000-000000000001";
const B = "5d4f5c68-0000-4000-8000-000000000002";
const PARAMS = Promise.resolve({ organization: "acme", project: "web" });
const open = async (query: Record<string, string | string[]>) =>
  render(await ComparePage({ params: PARAMS, searchParams: Promise.resolve(query) }));
const trace = (run_id: string) => ({ kind: "ok", trace: { run_id } });

describe("the comparison of two runs", () => {
  beforeEach(() => {
    mockMode = { content_mode: "on", offered: ["off", "on"] };
    fetchTrace.mockImplementation(async (_project, runId) => trace(runId));
  });
  afterEach(() => {
    cleanup();
    fetchTrace.mockReset();
  });

  it("reads both runs as metadata alone, the first chosen on the left", async () => {
    await open({ a: A, b: B });
    expect(screen.getByTestId("compared")).toHaveTextContent(`${A} ${B}`);
    expect(fetchTrace.mock.calls).toEqual([
      ["project_7bQx2mNv9BcK4dLp", A, false],
      ["project_7bQx2mNv9BcK4dLp", B, false],
    ]);
    expect(screen.getByTestId("compared")).toHaveAttribute("data-content", "true");
  });

  it("offers no messages where the project keeps no content", async () => {
    mockMode = { content_mode: "off", offered: ["off", "on"] };
    await open({ a: A, b: B });
    expect(screen.getByTestId("compared")).toHaveAttribute("data-content", "false");
  });

  it("reads nothing for a run that is not a UUID, one left out, or one trace twice", async () => {
    await open({ a: A });
    expect(screen.getByText("Choose two traces to compare.")).toBeInTheDocument();
    cleanup();
    await open({ a: A, b: "../admin" });
    expect(screen.getByText("Choose two traces to compare.")).toBeInTheDocument();
    cleanup();
    await open({ a: A, b: A.toUpperCase() });
    expect(screen.getByText("That is one trace twice.")).toBeInTheDocument();
    expect(fetchTrace).not.toHaveBeenCalled();
  });

  it("says which run is not in the project, and tells a refusal and an outage apart", async () => {
    fetchTrace.mockImplementation(async (_project, runId) => (runId === B ? { kind: "missing" } : trace(runId)));
    await open({ a: A, b: B });
    expect(screen.getByText("The second trace isn't in this project.")).toBeInTheDocument();
    cleanup();
    fetchTrace.mockImplementation(async () => ({ kind: "forbidden" }));
    await open({ a: A, b: B });
    expect(screen.getByTestId("denied")).toBeInTheDocument();
    cleanup();
    fetchTrace.mockImplementation(async () => ({ kind: "unavailable" }));
    await open({ a: A, b: B });
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
