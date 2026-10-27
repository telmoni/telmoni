import { beforeEach, describe, expect, it, vi } from "vitest";

let mockContext: unknown;
let mockProject: unknown;

vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => mockContext,
  fetchProjectBySlug: async () => mockProject,
}));

import { projectPage } from "./project-page";

beforeEach(() => {
  mockContext = { organizations: [], activeOrganizationId: "org_1" };
  mockProject = { id: "project_1", slug: "web", name: "Web", role: "owner" };
});

describe("projectPage", () => {
  it("hands a page its context and its project", async () => {
    expect(await projectPage("web")).toEqual({
      kind: "ok",
      gate: mockContext,
      project: mockProject,
    });
  });

  // Either read failing is an outage, which no page may draw as a refusal.
  it("is unavailable when the context or the project cannot be read", async () => {
    mockProject = null;
    expect(await projectPage("web")).toEqual({ kind: "unavailable" });
    mockProject = { id: "project_1", slug: "web", name: "Web", role: "owner" };
    mockContext = null;
    expect(await projectPage("web")).toEqual({ kind: "unavailable" });
  });
});
