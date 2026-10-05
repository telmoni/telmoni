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
let mockContextCalls: number;
let mockContext: {
  activeOrganizationId: string | null;
  organizations: Array<{
    organizationId: string;
    slug: string;
    name: string;
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
  activeOrganization: (ctx: NonNullable<typeof mockContext>) =>
    ctx.organizations.find((o) => o.organizationId === ctx.activeOrganizationId) ?? null,
}));

import ConsoleEntry from "./page";

beforeEach(() => {
  mockRedirect.mockReset();
  mockSession = { userId: "user_test" };
  mockContextCalls = 0;
  mockContext = {
    activeOrganizationId: "org_1",
    organizations: [{ organizationId: "org_1", slug: "acme", name: "Acme", role: "owner" }],
  };
});

describe("ConsoleEntry", () => {
  it("redirects to /auth/logout when session is absent", async () => {
    mockSession = null;
    await ConsoleEntry();
    expect(mockRedirect).toHaveBeenCalledWith("/auth/logout");
  });

  // ⚠ The overview, never a project: Vercel opens on a team's overview, and
  // the overview is where the projects are listed. Which organization is
  // auth's to say — the one the cookie remembers, else the person's default,
  // which a sign-in leaves it to by forgetting the cookie.
  it("lands on the overview of the organization auth answered", async () => {
    await ConsoleEntry();
    expect(mockContextCalls).toBe(1);
    expect(mockRedirect).toHaveBeenCalledWith("/acme");
  });

  // The projects page refuses a member, and the overview is open to all.
  it.each(["owner", "admin", "member"])(
    "lands an organization %s on the overview, not a page that might refuse them",
    async (role) => {
      mockContext = {
        activeOrganizationId: "org_1",
        organizations: [{ organizationId: "org_1", slug: "acme", name: "Acme", role }],
      };
      await ConsoleEntry();
      expect(mockRedirect).toHaveBeenCalledWith("/acme");
      expect(mockRedirect).not.toHaveBeenCalledWith("/acme/projects");
    },
  );

  // ⚠ Nothing is asked before the console opens, as Vercel and Cloudflare
  // ask nothing: an organization is provisioned already named after its
  // owner, at the URL that name reads as, and both are changed on Settings.
  it("opens a just-provisioned organization on its overview, asking nothing first", async () => {
    mockContext = {
      activeOrganizationId: "org_1",
      organizations: [
        {
          organizationId: "org_1",
          slug: "adas-organization",
          name: "Ada's organization",
          role: "owner",
        },
      ],
    };
    const ui = await ConsoleEntry();
    expect(ui).toBeUndefined();
    expect(mockRedirect).toHaveBeenCalledWith("/adas-organization");
  });

  it("renders ServiceUnavailable when auth could not answer", async () => {
    mockContext = null;
    const ui = await ConsoleEntry();
    render(ui as React.ReactElement);
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    expect(mockRedirect).not.toHaveBeenCalled();
  });

  // Sign-ups closed, and in no organization: there is no overview to land on,
  // and what there is for them is their invitations and their account.
  it("sends somebody in no organization to their account first", async () => {
    mockContext = { activeOrganizationId: null, organizations: [] };
    await ConsoleEntry();
    expect(mockRedirect).toHaveBeenNthCalledWith(1, "/account/notifications");
  });
});
