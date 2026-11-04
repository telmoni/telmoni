// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Score } from "@/lib/server/entities/scores";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => <div data-testid="page-header">{title}</div>,
}));
vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));
vi.mock("@/components/access-denied", () => ({
  AccessDenied: ({ what }: { what: string }) => <div data-testid="denied">{what}</div>,
}));
vi.mock("@/components/local-time", () => ({ LocalTime: ({ iso }: { iso: string }) => <span>{iso}</span> }));
vi.mock("@/lib/server/data", () => ({ activeOrganization: () => null }));
vi.mock("@/lib/server/project-page", () => ({
  projectPage: async () => ({ kind: "ok", gate: {}, project: { id: "project_1", slug: "web", role: "member" } }),
}));

let mockListing: unknown;
const asked: unknown[][] = [];
vi.mock("@/lib/server/entities/scores", () => ({
  fetchProjectScores: async (...args: unknown[]) => {
    asked.push(args);
    return mockListing;
  },
}));

import ScoresPage from "./page";

const score = (over: Partial<Score>): Score => ({
  score_id: "s1",
  target: "run",
  run_id: "11111111-1111-4111-8111-111111111111",
  name: "relevance",
  data_type: "number",
  value: 0.8,
  categories: [],
  source: "evaluator",
  given_at: "2026-11-03T10:00:00Z",
  ...over,
});

const page = async (query: Record<string, string> = {}) =>
  render(
    await ScoresPage({
      params: Promise.resolve({ organization: "acme", project: "web" }),
      searchParams: Promise.resolve(query),
    }),
  );

beforeEach(() => {
  asked.length = 0;
  mockListing = {
    kind: "ok",
    more: true,
    scores: [
      score({}),
      score({
        score_id: "s2",
        target: "session",
        run_id: undefined,
        session_id: "chat-9",
        name: "resolved",
        data_type: "boolean",
        value: 1,
        source: "sdk",
        given_at: "2026-11-03T09:00:00Z",
      }),
    ],
  };
});

describe("ScoresPage", () => {
  it("lists each score with what it says, what it scores and who gave it", async () => {
    await page();
    expect(screen.getByTestId("page-header")).toHaveTextContent("Scores");
    expect(screen.getByText("0.80")).toBeInTheDocument();
    expect(screen.getByText("yes")).toBeInTheDocument();
    expect(screen.getByText("by an evaluator")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "The trace relevance scores" })).toHaveAttribute(
      "href",
      "/acme/web/traces/11111111-1111-4111-8111-111111111111",
    );
    expect(screen.getByRole("link", { name: "The session resolved scores" })).toHaveAttribute(
      "href",
      "/acme/web/sessions/chat-9",
    );
    expect(screen.getByRole("link", { name: "relevance" })).toHaveAttribute("href", "/acme/web/scores?name=relevance");
  });

  it("narrows to one name, and pages it both ways while the lane says more follow", async () => {
    await page({ name: "relevance", page: "2" });
    expect(asked[0]).toEqual(["project_1", "relevance", 2]);
    expect(screen.getByRole("link", { name: "All scores" })).toHaveAttribute("href", "/acme/web/scores");
    expect(screen.getByRole("link", { name: "Older scores" })).toHaveAttribute(
      "href",
      "/acme/web/scores?name=relevance&page=3",
    );
    expect(screen.getByRole("link", { name: "Newer scores" })).toHaveAttribute(
      "href",
      "/acme/web/scores?name=relevance&page=1",
    );
  });

  it("reads a page that is no whole number as the first", async () => {
    await page({ page: "garbage" });
    expect(asked[0]).toEqual(["project_1", undefined, 0]);
    expect(screen.queryByRole("link", { name: "Newer scores" })).toBeNull();
  });

  it("says when there is none, and offers no older page", async () => {
    mockListing = { kind: "ok", scores: [], more: false };
    await page();
    expect(screen.getByText("No scores yet.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Older scores" })).toBeNull();
  });

  it("refuses where the lane does", async () => {
    mockListing = { kind: "forbidden" };
    await page();
    expect(screen.getByTestId("denied")).toHaveTextContent("this project's scores");
  });
});
