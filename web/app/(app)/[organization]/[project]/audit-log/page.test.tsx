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
let auditPagingAsked: unknown = null;

vi.mock("@/components/audit-pagination", () => ({
  AuditPagination: (props: { base: string; total: number; page: number; limit: number }) => (
    <div data-testid="pages">{JSON.stringify(props)}</div>
  ),
}));

vi.mock("@/lib/server/data", async () => {
  const { activeOrganization } = await vi.importActual<
    typeof import("@/lib/server/entities/organization")
  >("@/lib/server/entities/organization");
  return {
    activeOrganization,
    getServerContext: async () => mockContext,
    fetchProjectBySlug: async () => mockProject,
    fetchAuditEvents: async (projectId: string, paging: unknown) => {
      auditFetches += 1;
      auditAskedFor = projectId;
      auditPagingAsked = paging;
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
  auditPagingAsked = null;
  mockAuditEvents = { kind: "ok", events: [], total: 0, page: 1, limit: 50 };
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
    render(await AuditPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
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
    render(await AuditPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.queryByTestId("access-denied")).toBeNull();
    expect(screen.getByTestId("page-header")).toHaveTextContent("Audit log");
    expect(auditFetches).toBe(1);
    expect(auditAskedFor, "the log is read by the project's id, not its slug").toBe("proj_1");
  });

  it("renders the audit log for an owner caller", async () => {
    render(await AuditPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.queryByTestId("access-denied")).toBeNull();
    expect(screen.getByTestId("page-header")).toHaveTextContent("Audit log");
    expect(auditFetches).toBe(1);
  });

  it("reads the page and the rows its URL names, and spells its pages from its own path", async () => {
    render(await AuditPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(auditPagingAsked).toEqual({ page: undefined, limit: 50 });

    mockAuditEvents = { kind: "ok", events: [], total: 60, page: 3, limit: 25 };
    render(
      await AuditPage({
        params: PARAMS,
        searchParams: Promise.resolve({ page: "3", limit: "25" }),
      }),
    );
    expect(auditPagingAsked).toEqual({ page: 3, limit: 25 });
    expect(JSON.parse(screen.getByTestId("pages").textContent ?? "")).toEqual({
      base: "/acme/payments/audit-log",
      total: 60,
      page: 3,
      limit: 25,
    });
  });

  it("renders outage when server context is missing", async () => {
    mockContext = null;
    render(await AuditPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });

  // The project listing unread is an outage: an admin is never told they
  // have no access because auth could not answer.
  it("renders outage, never a refusal, when the project cannot be read", async () => {
    mockProject = null;
    render(await AuditPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    expect(screen.queryByTestId("access-denied")).toBeNull();
    expect(auditFetches).toBe(0);
  });
});
