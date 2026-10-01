// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mockNotFound = vi.fn();
vi.mock("next/navigation", () => ({
  notFound: () => {
    mockNotFound();
    throw new Error("NOT_FOUND");
  },
}));

let mockListing: unknown;
const mockFetchProjectListing = vi.fn(async () => mockListing);
vi.mock("@/lib/server/data", () => ({
  fetchProjectListing: () => mockFetchProjectListing(),
}));

let mockEverywhere: unknown;
vi.mock("@/lib/server/entities/projects", () => ({
  fetchProjectsEverywhere: async () => mockEverywhere,
}));

let mockContext: unknown;
vi.mock("@/lib/server/entities/organization", () => ({
  getServerContext: async () => mockContext,
}));

vi.mock("../actions", () => ({
  switchActiveOrganizationAction: vi.fn(),
}));

import ProjectLayout from "./layout";

async function renderLayout(projectId: string) {
  const ui = await ProjectLayout({
    children: <div data-testid="page">page</div>,
    params: Promise.resolve({ projectId }),
  });
  render(ui as React.ReactElement);
}

describe("ProjectLayout", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListing = {
      kind: "ok",
      projects: [{ id: "project_in_context", name: "Core", role: "admin" }],
    };
    mockEverywhere = { kind: "ok", projects: [] };
    mockContext = { organizations: [] };
  });

  it("offers the organization a project moved to instead of a 404", async () => {
    mockEverywhere = {
      kind: "ok",
      projects: [
        {
          id: "project_moved",
          name: "Payments",
          role: "admin",
          organizationId: "org_new",
          organizationName: "Acme",
          organizationOwnerEmail: null,
        },
      ],
    };
    mockContext = { organizations: [{ organizationId: "org_new" }] };
    await renderLayout("project_moved");
    expect(screen.getByTestId("project-elsewhere")).toHaveTextContent("Payments is in Acme.");
    expect(screen.getByRole("button", { name: "Open it in Acme" })).toBeInTheDocument();
    expect(screen.queryByTestId("page")).not.toBeInTheDocument();
    expect(mockNotFound).not.toHaveBeenCalled();
  });

  it("answers 404 rather than offer a switch to the organization already active", async () => {
    mockEverywhere = {
      kind: "ok",
      projects: [
        {
          id: "project_here",
          name: "Payments",
          role: "admin",
          organizationId: "org_here",
          organizationName: "Acme",
          organizationOwnerEmail: null,
        },
      ],
    };
    mockContext = { activeOrganizationId: "org_here", organizations: [{ organizationId: "org_here" }] };
    await expect(renderLayout("project_here")).rejects.toThrow("NOT_FOUND");
    expect(mockNotFound).toHaveBeenCalledTimes(1);
  });

  it("answers 404 for a project reached only by a seat in an organization the person is not in", async () => {
    mockEverywhere = {
      kind: "ok",
      projects: [
        {
          id: "project_seat",
          name: "Shared",
          role: "member",
          organizationId: "org_theirs",
          organizationName: null,
          organizationOwnerEmail: "owner@example.com",
        },
      ],
    };
    await expect(renderLayout("project_seat")).rejects.toThrow("NOT_FOUND");
    expect(mockNotFound).toHaveBeenCalledTimes(1);
  });

  it("renders a project the active organization lists", async () => {
    await renderLayout("project_in_context");
    expect(screen.getByTestId("page")).toBeInTheDocument();
    expect(mockNotFound).not.toHaveBeenCalled();
  });

  it("answers 404 for a project the active organization does not list", async () => {
    await expect(renderLayout("project_other_org")).rejects.toThrow("NOT_FOUND");
    expect(mockNotFound).toHaveBeenCalledTimes(1);
  });

  it("answers 404 for the organization segment, which names no project", async () => {
    for (const segment of ["organization", "org"]) {
      await expect(renderLayout(segment)).rejects.toThrow("NOT_FOUND");
    }
    expect(mockFetchProjectListing).toHaveBeenCalled();
    expect(mockNotFound).toHaveBeenCalledTimes(2);
  });

  it("does not answer 404 for a listing auth could not produce", async () => {
    mockListing = { kind: "unavailable" };
    await renderLayout("project_in_context");
    expect(screen.getByTestId("page")).toBeInTheDocument();
    expect(mockNotFound).not.toHaveBeenCalled();
  });
});
