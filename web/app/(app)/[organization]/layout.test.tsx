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

let mockContext: unknown;
vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => mockContext,
}));

let mockSeed: unknown;
vi.mock("@/lib/server/store-seed", () => ({
  storeSeed: async () => mockSeed,
}));

// The seed the layout draws is what the store follows into this organization;
// which one it is for is the assertion.
vi.mock("@/lib/store/provider", () => ({
  StoreSeed: ({ activeOrganizationId }: { activeOrganizationId: string | null }) => (
    <div data-testid="seed" data-organization={activeOrganizationId ?? ""} />
  ),
}));

// The path as the proxy hands it on, which is what a redirect is spelled from.
let mockPath: string | null;
vi.mock("next/headers", () => ({
  headers: async () => new Headers(mockPath ? { "x-telmoni-path": mockPath } : {}),
}));

import OrganizationLayout from "./layout";

async function renderLayout(organization: string) {
  const ui = await OrganizationLayout({
    children: <div data-testid="page">page</div>,
    params: Promise.resolve({ organization }),
  });
  render(ui as React.ReactElement);
}

const ACME = { organizationId: "org_acme", slug: "acme", name: "Acme", role: "owner" };
const GLOBEX = { organizationId: "org_globex", slug: "globex", name: "Globex", role: "member" };
// Provisioned at first sign-in and not yet named: its slug is a placeholder.
const FRESH = { organizationId: "org_fresh", slug: "org-k3x9qz1a2b", name: null, role: "owner" };

// `/me` as auth answers it when the path names `active`.
function standingIn(active: string | null) {
  mockContext = { organizations: [ACME, GLOBEX], activeOrganizationId: active };
  mockSeed = { activeOrganizationId: active };
}

describe("OrganizationLayout", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    standingIn("org_acme");
    mockPath = null;
  });

  it("renders the page, and seeds the store, for the organization the path names", async () => {
    await renderLayout("acme");
    expect(screen.getByTestId("page")).toBeInTheDocument();
    expect(screen.getByTestId("seed")).toHaveAttribute("data-organization", "org_acme");
    expect(mockRedirect).not.toHaveBeenCalled();
    expect(mockNotFound).not.toHaveBeenCalled();
  });

  // Auth answers a path that names somebody else's organization with one of
  // the person's own. Rendering that under the stranger's slug would show
  // their own data at an address that says it is another's.
  it("answers 404 for an organization the person is not in", async () => {
    await expect(renderLayout("initech")).rejects.toThrow("NOT_FOUND");
    expect(mockNotFound).toHaveBeenCalledTimes(1);
    expect(mockRedirect).not.toHaveBeenCalled();
  });

  it.each(["account", "organization", "console", "api"])(
    "answers 404 for %s, which no organization goes by",
    async (word) => {
      await expect(renderLayout(word)).rejects.toThrow("NOT_FOUND");
    },
  );

  // ⚠ The invariant every page below leans on: the request acts in the
  // organization the path names. A slug the person IS in, answered with
  // another organization, must not render as if it had been honoured.
  it("answers 404 when auth resolved another organization than the path names", async () => {
    standingIn("org_acme");
    await expect(renderLayout("globex")).rejects.toThrow("NOT_FOUND");
    expect(screen.queryByTestId("page")).toBeNull();
  });

  // An id is how a link kept longer than a page may name an organization: it
  // cannot follow a URL change, so the layout does.
  it("redirects an organization's id to its slug, keeping the page and the query", async () => {
    mockPath = "/org_globex/billing?plan=team";
    await expect(renderLayout("org_globex")).rejects.toThrow(
      "REDIRECT:/globex/billing?plan=team",
    );
    expect(mockNotFound).not.toHaveBeenCalled();
  });

  it("redirects an id on a project's path, leaving the project's segment as it is", async () => {
    mockPath = "/org_acme/project_7bQx2mNv9BcK4dLp/audit-log";
    await expect(renderLayout("org_acme")).rejects.toThrow(
      "REDIRECT:/acme/project_7bQx2mNv9BcK4dLp/audit-log",
    );
  });

  it("redirects a slug typed with a capital to the one it is", async () => {
    mockPath = "/Acme/web";
    await expect(renderLayout("Acme")).rejects.toThrow("REDIRECT:/acme/web");
  });

  it("answers 404 for the id of an organization the person is not in", async () => {
    mockPath = "/org_initech";
    await expect(renderLayout("org_initech")).rejects.toThrow("NOT_FOUND");
    expect(mockRedirect).not.toHaveBeenCalled();
  });

  // ⚠ The owner names their organization before the console opens to them,
  // on `/console`: nothing is drawn under the placeholder slug, which looks
  // like an id and is nobody's choice of address.
  it("sends the owner of an organization not yet named to name it", async () => {
    mockContext = { organizations: [FRESH], activeOrganizationId: "org_fresh" };
    mockSeed = { activeOrganizationId: "org_fresh" };
    await expect(renderLayout("org-k3x9qz1a2b")).rejects.toThrow("REDIRECT:/console?organization=org_fresh");
    expect(mockNotFound).not.toHaveBeenCalled();
  });

  // Nobody but the owner is in an unnamed organization (auth refuses its
  // invitations), so this is the operator's case: an organization left
  // unnamed renders for a member, under the label the console gives one.
  it("renders an unnamed organization for somebody who is not its owner", async () => {
    mockContext = {
      organizations: [{ ...FRESH, role: "member" }],
      activeOrganizationId: "org_fresh",
    };
    mockSeed = { activeOrganizationId: "org_fresh" };
    await renderLayout("org-k3x9qz1a2b");
    expect(screen.getByTestId("page")).toBeInTheDocument();
    expect(mockRedirect).not.toHaveBeenCalled();
  });

  // An outage is the page's to say, as each one does: a 404 here would tell
  // somebody their organization was gone.
  it("leaves the page to say so when auth does not answer", async () => {
    mockContext = null;
    await renderLayout("acme");
    expect(screen.getByTestId("page")).toBeInTheDocument();
    expect(screen.queryByTestId("seed")).toBeNull();
    expect(mockNotFound).not.toHaveBeenCalled();
  });
});
