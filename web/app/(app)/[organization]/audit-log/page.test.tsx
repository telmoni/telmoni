// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { IdentityContext } from "@/lib/server/entities/identity-context";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => (
    <div data-testid="page-header">{title}</div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

let mockContext: unknown;
let mockAuditEvents: unknown;
let mockIdentityContext: IdentityContext | null = null;
let auditFetches = 0;

vi.mock("@/lib/server/data", async () => {
  const { activeOrganization } = await vi.importActual<
    typeof import("@/lib/server/entities/organization")
  >("@/lib/server/entities/organization");
  return {
    activeOrganization,
    getServerContext: async () => mockContext,
    fetchOrganizationAudit: async () => {
      auditFetches += 1;
      return mockAuditEvents;
    },
    identityContext: async () => mockIdentityContext,
  };
});

import OrganizationAuditPage from "./page";

function standingAs(
  role: IdentityContext["role"],
  userId: string,
  ownerEmail = "owner@example.com",
) {
  mockIdentityContext = {
    userId,
    organizationId: "org_acme",
    role,
    accessToken: "at_1",
  };
  mockContext = {
    organizations: [
      { organizationId: "org_acme", slug: "acme", name: "Acme", ownerEmail, role },
    ],
    activeOrganizationId: "org_acme",
    memberships: [],
    incomingInvites: [],
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  auditFetches = 0;
  mockAuditEvents = { kind: "ok", events: [] };
  standingAs("owner", "user_owner");
});

describe("OrganizationAuditPage", () => {
  it("refuses the page to an organization member and names who to ask", async () => {
    standingAs("member", "user_member");
    render(await OrganizationAuditPage());
    expect(screen.getByTestId("access-denied")).toBeInTheDocument();
    expect(document.body.textContent).toMatch(/this organization.s audit log/i);
    expect(document.body.textContent).toMatch(
      /contact owner@example\.com, the organization.s owner, to request access/i,
    );
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
      "href",
      "/acme",
    );
    expect(
      auditFetches,
      "the page asked auth for an audit log it already knew the member could not read",
    ).toBe(0);
  });

  it("renders the audit log for an admin caller", async () => {
    standingAs("admin", "user_admin");
    render(await OrganizationAuditPage());
    expect(screen.queryByTestId("access-denied")).toBeNull();
    expect(screen.getByTestId("page-header")).toHaveTextContent("Audit log");
    expect(auditFetches).toBe(1);
  });

  it("renders the audit log for an owner caller", async () => {
    render(await OrganizationAuditPage());
    expect(screen.queryByTestId("access-denied")).toBeNull();
    expect(screen.getByTestId("page-header")).toHaveTextContent("Audit log");
    expect(auditFetches).toBe(1);
  });

  it("renders outage when server context is missing", async () => {
    mockContext = null;
    render(await OrganizationAuditPage());
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
