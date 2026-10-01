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

let mockSession: { userId: string } | null;
let mockListing:
  | { kind: "ok"; projects: Array<{ id: string; name: string; role: string | null }> }
  | { kind: "unavailable" };
let mockContextCalls: number;
let mockContext: {
  activeOrganizationId: string | null;
  organizations: Array<{ organizationId: string; role: string }>;
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

beforeEach(() => {
  mockRedirect.mockReset();
  mockSession = { userId: "user_test" };
  mockListing = {
    kind: "ok",
    projects: [{ id: "project_1234567890abcdef", name: "Personal project", role: "owner" }],
  };
  mockContextCalls = 0;
  mockContext = {
    activeOrganizationId: "org_1",
    organizations: [{ organizationId: "org_1", role: "owner" }],
  };
});

describe("ConsoleEntry", () => {
  it("redirects to /auth/logout when session is absent", async () => {
    mockSession = null;
    await ConsoleEntry();
    expect(mockRedirect).toHaveBeenCalledWith("/auth/logout");
  });

  it("calls getServerContext for fallback provisioning and redirects to first project", async () => {
    await ConsoleEntry();
    expect(mockContextCalls).toBe(1);
    expect(mockRedirect).toHaveBeenCalledWith("/project_1234567890abcdef");
  });

  it("renders ServiceUnavailable when the listing could not be read", async () => {
    mockListing = { kind: "unavailable" };
    const ui = await ConsoleEntry();
    render(ui as React.ReactElement);
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    expect(mockRedirect).not.toHaveBeenCalled();
  });

  // Sign-ups closed, and in no organization: there is no project to land on,
  // and what there is for them is their invitations and their account.
  it("sends somebody in no organization to their account first", async () => {
    mockContext = { activeOrganizationId: null, organizations: [] };
    await ConsoleEntry();
    expect(mockRedirect).toHaveBeenNthCalledWith(1, "/account/notifications");
  });

  it("sends an owner whose organization lists nothing to its projects page", async () => {
    mockListing = { kind: "ok", projects: [] };
    await ConsoleEntry();
    expect(mockRedirect).toHaveBeenCalledWith("/organization/projects");
  });

  // The projects page refuses a member, so this was an ACCESS DENIED landing
  // for somebody whose only project there had been handed away.
  it("sends a member with nothing to open to the overview, not a page that refuses them", async () => {
    mockListing = { kind: "ok", projects: [] };
    mockContext = {
      activeOrganizationId: "org_1",
      organizations: [{ organizationId: "org_1", role: "member" }],
    };
    await ConsoleEntry();
    expect(mockRedirect).toHaveBeenCalledWith("/organization");
    expect(mockRedirect).not.toHaveBeenCalledWith("/organization/projects");
  });
});
