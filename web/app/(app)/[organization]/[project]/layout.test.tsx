// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mockNotFound = vi.fn();
const mockRedirect = vi.fn();
vi.mock("next/navigation", () => ({
  notFound: () => {
    mockNotFound();
    throw new Error("NOT_FOUND");
  },
  redirect: (to: string) => {
    mockRedirect(to);
    throw new Error(`REDIRECT:${to}`);
  },
}));

let mockListing: unknown;
let mockEverywhere: unknown;
let mockContext: unknown;
vi.mock("@/lib/server/data", () => ({
  fetchProjectListing: async () => mockListing,
  fetchProjectsEverywhere: async () => mockEverywhere,
  getServerContext: async () => mockContext,
}));

// The path as the proxy hands it on, which is what a redirect is spelled from.
let mockPath: string | null;
vi.mock("next/headers", () => ({
  headers: async () => new Headers(mockPath ? { "x-telmoni-path": mockPath } : {}),
}));

import ProjectLayout from "./layout";

async function renderLayout(project: string, organization = "acme") {
  const ui = await ProjectLayout({
    children: <div data-testid="page">page</div>,
    params: Promise.resolve({ organization, project }),
  });
  render(ui as React.ReactElement);
}

const moved = (over: Record<string, unknown> = {}) => ({
  id: "project_moved",
  slug: "payments",
  name: "Payments",
  role: "admin",
  organizationId: "org_new",
  organizationSlug: "globex",
  organizationName: "Globex",
  ...over,
});

describe("ProjectLayout", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListing = {
      kind: "ok",
      projects: [{ id: "project_in_context", slug: "core", name: "Core", role: "admin" }],
    };
    mockEverywhere = { kind: "ok", projects: [] };
    mockContext = { organizations: [{ organizationId: "org_here", slug: "acme" }] };
    mockPath = null;
  });

  it("renders the page for a project of the organization the path names", async () => {
    await renderLayout("core");
    expect(screen.getByTestId("page")).toBeInTheDocument();
    expect(mockRedirect).not.toHaveBeenCalled();
    expect(mockNotFound).not.toHaveBeenCalled();
  });

  it("answers 404 for a slug the organization has no project by", async () => {
    await expect(renderLayout("gone")).rejects.toThrow("NOT_FOUND");
    expect(mockNotFound).toHaveBeenCalledTimes(1);
  });

  // Two organizations may each have a project of the same name, so a slug says
  // nothing about a project anywhere but in the organization the path names.
  it("answers 404 rather than guess at a same-named project in another organization", async () => {
    mockEverywhere = { kind: "ok", projects: [moved({ slug: "default-project" })] };
    mockContext = {
      organizations: [
        { organizationId: "org_here", slug: "acme" },
        { organizationId: "org_new", slug: "globex" },
      ],
    };
    await expect(renderLayout("default-project")).rejects.toThrow("NOT_FOUND");
  });

  // An id is how the connect handshake's cookie names a project, and how a
  // link kept longer than a page may: neither can follow a rename, so the
  // layout does.
  it("redirects a project's id to the slug it goes by, keeping the page and the query", async () => {
    mockPath = "/acme/project_in_context/connectors?connected=slack";
    await expect(renderLayout("project_in_context")).rejects.toThrow(
      "REDIRECT:/acme/core/connectors?connected=slack",
    );
  });

  it("redirects a slug typed with a capital to the one it is", async () => {
    mockPath = "/acme/Core/api-keys";
    await expect(renderLayout("Core")).rejects.toThrow("REDIRECT:/acme/core/api-keys");
  });

  // A project handed to another organization since the link was made: the id
  // still names it, and the person keeps a seat on it there.
  it("follows a project's id into the organization it was handed to", async () => {
    mockEverywhere = { kind: "ok", projects: [moved()] };
    mockContext = {
      organizations: [
        { organizationId: "org_here", slug: "acme" },
        { organizationId: "org_new", slug: "globex" },
      ],
    };
    mockPath = "/acme/project_moved/audit-log";
    await expect(renderLayout("project_moved")).rejects.toThrow(
      "REDIRECT:/globex/payments/audit-log",
    );
    expect(mockNotFound).not.toHaveBeenCalled();
  });

  it("answers 404 for a project reached only by a seat in an organization the person is not in", async () => {
    mockEverywhere = { kind: "ok", projects: [moved({ organizationId: "org_other" })] };
    mockPath = "/acme/project_moved";
    await expect(renderLayout("project_moved")).rejects.toThrow("NOT_FOUND");
    expect(mockNotFound).toHaveBeenCalledTimes(1);
    expect(mockRedirect).not.toHaveBeenCalled();
  });

  it("answers 404 for an id nobody the person can open goes by", async () => {
    await expect(renderLayout("project_unknown")).rejects.toThrow("NOT_FOUND");
  });

  // An outage is the page's to say, as each one does: a 404 here would tell
  // somebody their project was gone.
  it("leaves the page to say so when the listing cannot be read", async () => {
    mockListing = { kind: "unavailable" };
    await renderLayout("core");
    expect(screen.getByTestId("page")).toBeInTheDocument();
    expect(mockNotFound).not.toHaveBeenCalled();
  });
});
