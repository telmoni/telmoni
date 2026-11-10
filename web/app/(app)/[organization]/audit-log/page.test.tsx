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
let auditAskedFor: unknown = null;

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
    fetchOrganizationAudit: async (paging: unknown) => {
      auditFetches += 1;
      auditAskedFor = paging;
      return mockAuditEvents;
    },
    identityContext: async () => mockIdentityContext,
  };
});

import OrganizationAuditPage from "./page";

const page = (query: Record<string, string> = {}) =>
  OrganizationAuditPage({
    params: Promise.resolve({ organization: "acme" }),
    searchParams: Promise.resolve(query),
  });

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
  auditAskedFor = null;
  mockAuditEvents = { kind: "ok", events: [], total: 0, page: 1, limit: 50 };
  standingAs("owner", "user_owner");
});

describe("OrganizationAuditPage", () => {
  it("refuses the page to an organization member and names who to ask", async () => {
    standingAs("member", "user_member");
    render(await page());
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
    render(await page());
    expect(screen.queryByTestId("access-denied")).toBeNull();
    expect(screen.getByTestId("page-header")).toHaveTextContent("Audit log");
    expect(auditFetches).toBe(1);
  });

  it("renders the audit log for an owner caller", async () => {
    render(await page());
    expect(screen.queryByTestId("access-denied")).toBeNull();
    expect(screen.getByTestId("page-header")).toHaveTextContent("Audit log");
    expect(auditFetches).toBe(1);
  });

  it("reads the page and the rows its URL names, the first page of fifty where it names none", async () => {
    render(await page());
    expect(auditAskedFor).toEqual({ page: undefined, limit: 50 });
    render(await page({ page: "2", limit: "25" }));
    expect(auditAskedFor).toEqual({ page: 2, limit: 25 });
    render(await page({ page: "0", limit: "7" }));
    expect(auditAskedFor, "a page or a row count the footer does not offer").toEqual({
      page: undefined,
      limit: 50,
    });
  });

  it("draws the pages under its rows, and under a page past the last, never under an empty log", async () => {
    render(await page());
    expect(screen.queryByTestId("pages")).toBeNull();
    expect(document.body.textContent).toContain("No events yet.");

    mockAuditEvents = { kind: "ok", events: [], total: 3, page: 9, limit: 50 };
    render(await page({ page: "9" }));
    expect(document.body.textContent).toContain("No events on this page.");
    expect(JSON.parse(screen.getByTestId("pages").textContent ?? "")).toEqual({
      base: "/acme/audit-log",
      total: 3,
      page: 9,
      limit: 50,
    });
  });

  it("renders outage when server context is missing", async () => {
    mockContext = null;
    render(await page());
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
