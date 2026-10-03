// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mockRedirect = vi.fn();
vi.mock("next/navigation", () => ({
  redirect: (url: string) => mockRedirect(url),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));
vi.mock("@/components/paper-shell", () => ({
  PaperShell: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));
vi.mock("./_name-organization", () => ({
  NameOrganizationForm: ({ organizationId }: { organizationId: string }) => (
    <form data-testid="name-form" data-organization-id={organizationId} />
  ),
}));

let mockSession: { userId: string } | null;
let mockListing:
  | {
      kind: "ok";
      projects: Array<{ id: string; slug: string; name: string; role: string | null }>;
    }
  | { kind: "unavailable" };
let mockContextCalls: number;
let mockContext: {
  activeOrganizationId: string | null;
  organizations: Array<{
    organizationId: string;
    slug: string;
    name: string | null;
    role: string;
  }>;
} | null;

vi.mock("@/lib/server/session", () => ({
  getServerSession: async () => mockSession,
}));

vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => {
    mockContextCalls++;
    return mockContext;
  },
  fetchProjectListing: async () => mockListing,
  activeOrganization: (ctx: NonNullable<typeof mockContext>) =>
    ctx.organizations.find((o) => o.organizationId === ctx.activeOrganizationId) ?? null,
}));

import ConsoleEntry from "./page";

// The page as Next calls it: `?organization=` is what the organization layout
// adds when it sends an owner here to name one.
const entry = (query: { organization?: string } = {}) =>
  ConsoleEntry({ searchParams: Promise.resolve(query) });

beforeEach(() => {
  mockRedirect.mockReset();
  mockSession = { userId: "user_test" };
  mockListing = {
    kind: "ok",
    projects: [
      {
        id: "project_1234567890abcdef",
        slug: "personal-project",
        name: "Personal project",
        role: "owner",
      },
    ],
  };
  mockContextCalls = 0;
  mockContext = {
    activeOrganizationId: "org_1",
    organizations: [{ organizationId: "org_1", slug: "acme", name: "Acme", role: "owner" }],
  };
});

describe("ConsoleEntry", () => {
  it("redirects to /auth/logout when session is absent", async () => {
    mockSession = null;
    await entry();
    expect(mockRedirect).toHaveBeenCalledWith("/auth/logout");
  });

  it("redirects a named organization with projects to its first project", async () => {
    await entry();
    expect(mockContextCalls).toBe(1);
    // By the slugs, the project's under its organization's: the path names both.
    expect(mockRedirect).toHaveBeenCalledWith("/acme/personal-project");
  });

  it("renders ServiceUnavailable when the listing could not be read", async () => {
    mockListing = { kind: "unavailable" };
    const ui = await entry();
    render(ui as React.ReactElement);
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    expect(mockRedirect).not.toHaveBeenCalled();
  });

  // Sign-ups closed, and in no organization: there is no project to land on,
  // and what there is for them is their invitations and their account.
  it("sends somebody in no organization to their account first", async () => {
    mockContext = { activeOrganizationId: null, organizations: [] };
    await entry();
    expect(mockRedirect).toHaveBeenNthCalledWith(1, "/account/notifications");
  });

  it("sends an owner whose organization lists nothing to its projects page", async () => {
    mockListing = { kind: "ok", projects: [] };
    await entry();
    expect(mockRedirect).toHaveBeenCalledWith("/acme/~/projects");
  });

  // The projects page refuses a member, so this was an ACCESS DENIED landing
  // for somebody whose only project there had been handed away.
  it("sends a member with nothing to open to the overview, not a page that refuses them", async () => {
    mockListing = { kind: "ok", projects: [] };
    mockContext = {
      activeOrganizationId: "org_1",
      organizations: [{ organizationId: "org_1", slug: "acme", name: "Acme", role: "member" }],
    };
    await entry();
    expect(mockRedirect).toHaveBeenCalledWith("/acme");
    expect(mockRedirect).not.toHaveBeenCalledWith("/acme/~/projects");
  });

  // ⚠ The first thing a new owner sees is the question, not a console under a
  // placeholder slug: provisioned at first sign-in, the organization has no
  // name yet, and its address follows from the one they give it.
  it("asks the owner to name an organization that has none, before anything else", async () => {
    mockContext = {
      activeOrganizationId: "org_1",
      organizations: [{ organizationId: "org_1", slug: "org-k3x9qz1a2b", name: null, role: "owner" }],
    };
    const ui = await entry();
    render(ui as React.ReactElement);
    expect(screen.getByTestId("name-organization")).toBeInTheDocument();
    expect(screen.getByTestId("name-form")).toHaveAttribute("data-organization-id", "org_1");
    expect(mockRedirect).not.toHaveBeenCalled();
  });

  // ⚠ The owner opened an unnamed organization of theirs from inside another
  // one: this path names no organization, so `/me` answers the cookie's — the
  // named one — and the question has to be asked of the one the layout sent
  // them from, or it is never asked at all.
  it("asks for the name of the organization the layout sent the owner from, not the cookie's", async () => {
    mockContext = {
      activeOrganizationId: "org_1",
      organizations: [
        { organizationId: "org_1", slug: "acme", name: "Acme", role: "owner" },
        { organizationId: "org_2", slug: "org-k3x9qz1a2b", name: null, role: "owner" },
      ],
    };
    const ui = await entry({ organization: "org_2" });
    render(ui as React.ReactElement);
    expect(screen.getByTestId("name-form")).toHaveAttribute("data-organization-id", "org_2");
    expect(mockRedirect).not.toHaveBeenCalled();
  });

  it("ignores a query naming an organization the person is not in", async () => {
    await entry({ organization: "org_stranger" });
    expect(mockRedirect).toHaveBeenCalledWith("/acme/personal-project");
  });

  // Nobody but the owner is in an unnamed organization; should an operator
  // leave one so, a member is sent on as to any other.
  it("sends somebody who is not the owner of an unnamed organization on as usual", async () => {
    mockContext = {
      activeOrganizationId: "org_1",
      organizations: [{ organizationId: "org_1", slug: "org-k3x9qz1a2b", name: null, role: "member" }],
    };
    await entry();
    expect(mockRedirect).toHaveBeenCalledWith("/org-k3x9qz1a2b/personal-project");
  });
});
