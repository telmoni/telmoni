// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { Role } from "@/lib/types/enums";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => (
    <div data-testid="page-header">{title}</div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

let mockContext: unknown;
let mockProject: unknown;
let mockAuditEvents: unknown;
let auditFetches = 0;
let auditAskedFor: string | null = null;

vi.mock("@/lib/server/data", async () => {
  const { activeOrganization } = await vi.importActual<
    typeof import("@/lib/server/entities/organization")
  >("@/lib/server/entities/organization");
  return {
    activeOrganization,
    getServerContext: async () => mockContext,
    fetchProjectBySlug: async () => mockProject,
    fetchAuditEvents: async (projectId: string) => {
      auditFetches += 1;
      auditAskedFor = projectId;
      return mockAuditEvents;
    },
  };
});

import AuditPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "payments" });

beforeEach(() => {
  vi.clearAllMocks();
  auditFetches = 0;
  auditAskedFor = null;
  mockAuditEvents = { kind: "ok", events: [] };
  mockContext = {
    organizations: [
      {
        organizationId: "org_acme",
        slug: "acme",
        name: "Acme",
        ownerEmail: "owner@example.com",
        role: "owner",
      },
    ],
    activeOrganizationId: "org_acme",
    memberships: [],
    incomingInvites: [],
  };
  mockProject = {
    id: "proj_1",
    slug: "payments",
    name: "Payments",
    role: Role.Owner,
  };
});

describe("AuditPage (project)", () => {
  it("refuses the page to a project member and names who to ask", async () => {
    mockProject = {
      id: "proj_1",
      name: "Payments",
      role: Role.Member,
    };
    render(await AuditPage({ params: PARAMS }));
    expect(screen.getByTestId("access-denied")).toBeInTheDocument();
    expect(document.body.textContent).toMatch(/this project.s audit log/i);
    expect(document.body.textContent).toMatch(
      /contact owner@example\.com, the organization.s owner, to request access/i,
    );
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
      "href",
      "/acme/payments",
    );
    expect(
      auditFetches,
      "the page asked auth for an audit log it already knew the member could not read",
    ).toBe(0);
  });

  it("renders the audit log for an admin caller", async () => {
    mockProject = {
      id: "proj_1",
      name: "Payments",
      role: Role.Admin,
    };
    render(await AuditPage({ params: PARAMS }));
    expect(screen.queryByTestId("access-denied")).toBeNull();
    expect(screen.getByTestId("page-header")).toHaveTextContent("Audit log");
    expect(auditFetches).toBe(1);
    expect(auditAskedFor, "the log is read by the project's id, not its slug").toBe("proj_1");
  });

  it("renders the audit log for an owner caller", async () => {
    render(await AuditPage({ params: PARAMS }));
    expect(screen.queryByTestId("access-denied")).toBeNull();
    expect(screen.getByTestId("page-header")).toHaveTextContent("Audit log");
    expect(auditFetches).toBe(1);
  });

  it("renders outage when server context is missing", async () => {
    mockContext = null;
    render(await AuditPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
