// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { IdentityContext } from "@/lib/server/entities/identity-context";
import type { OrganizationEntry } from "@/lib/server/entities/organization";
import { Role } from "@/lib/types/enums";

vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh: vi.fn() }),
}));

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title: string; action?: React.ReactNode }) => (
    <div data-testid="page-header">
      {title}
      {action}
    </div>
  ),
}));

vi.mock("@/components/create-project", () => ({
  CreateProjectAction: () => <div data-testid="create-project-action" />,
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

let mockContext: unknown;
let mockProjects: unknown[] = [];
let mockIdent: IdentityContext | null = null;

// The real `activeOrganization`: the owner each shared row credits comes off it.
vi.mock("@/lib/server/data", async () => {
  const { activeOrganization } = await vi.importActual<
    typeof import("@/lib/server/entities/organization")
  >("@/lib/server/entities/organization");
  return {
    activeOrganization,
    getServerContext: async () => mockContext,
    fetchProjects: async () => mockProjects,
    identityContext: async () => mockIdent,
  };
});

import OrganizationProjectsPage from "./page";

const OWNED: OrganizationEntry = {
  organizationId: "org_owned",
  name: null,
  ownerEmail: "owner@example.com",
  ownerDisplayName: null,
  role: "owner",
};

// Alex Founder's organization, which the caller stands in at `role`.
function founders(role: OrganizationEntry["role"]): OrganizationEntry {
  return {
    organizationId: "org_founders",
    name: null,
    ownerEmail: "alex@example.test",
    ownerDisplayName: "Alex Founder",
    role,
  };
}

function standingIn(
  userId: string,
  organization: OrganizationEntry,
  memberships: { projectId: string; organizationId: string; role: Role | null }[] = [],
) {
  mockIdent = {
    userId,
    organizationId: organization.organizationId,
    role: organization.role,
    accessToken: "at_1",
  };
  mockContext = {
    organizations: [organization],
    activeOrganizationId: organization.organizationId,
    memberships,
    incomingInvites: [],
  };
}

describe("OrganizationProjectsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    standingIn("user_owner", OWNED);
    mockProjects = [
      {
        id: "project_own_1",
        name: "Personal Project",
        role: Role.Owner,
      },
    ];
  });

  it("renders outage when server context is unavailable", async () => {
    mockContext = null;
    render(
      await OrganizationProjectsPage(),
    );
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });

  it("renders owned projects with ID and Open links", async () => {
    render(
      await OrganizationProjectsPage(),
    );

    expect(screen.getByTestId("page-header")).toHaveTextContent("Projects");
    expect(screen.getByTestId("create-project-action")).toBeInTheDocument();
    expect(screen.getByText("Personal Project")).toBeInTheDocument();
    expect(screen.getByText("project_own_1")).toBeInTheDocument();
    expect(screen.getByText("Owner")).toBeInTheDocument();

    const openLink = screen.getByRole("link", { name: /open/i });
    expect(openLink.getAttribute("href")).toBe("/project_own_1");
  });

  it("draws no shared section for somebody who administers the organization", async () => {
    render(
      await OrganizationProjectsPage(),
    );

    expect(screen.queryByTestId("projects-shared-with-you")).toBeNull();
    expect(
      screen.queryByText("No project in this organization has been shared with you."),
    ).toBeNull();
    expect(screen.getByText("Owned by this organization")).toBeInTheDocument();
    expect(screen.getByText("Personal Project")).toBeInTheDocument();
  });

  it("refuses the page to an organization member and names who to ask", async () => {
    standingIn("user_member", founders("member"), [
      { projectId: "project_shared_99", organizationId: "org_founders", role: Role.Admin },
    ]);

    mockProjects = [
      {
        id: "project_shared_99",
        name: "Core Platform",
        role: Role.Admin,
      },
    ];

    render(
      await OrganizationProjectsPage(),
    );

    expect(screen.getByTestId("access-denied")).toBeInTheDocument();
    expect(document.body.textContent).toMatch(/this organization.s projects/i);
    expect(document.body.textContent).toMatch(
      /contact Alex Founder, the organization.s owner, to request access/i,
    );
  });

  it("files an organization admin's projects under the organization's own, at their role", async () => {
    standingIn("user_admin", founders("admin"));
    mockProjects = [
      { id: "project_own_1", name: "Default Project", role: Role.Admin },
      { id: "project_growth", name: "Growth", role: Role.Admin },
    ];

    render(
      await OrganizationProjectsPage(),
    );

    expect(screen.queryByTestId("projects-shared-with-you")).toBeNull();
    expect(screen.getByText("Growth")).toBeInTheDocument();
    expect(screen.getAllByText("Admin")).toHaveLength(2);
    expect(screen.queryByText("No projects yet.")).toBeNull();
    expect(screen.queryByRole("button", { name: /leave/i })).toBeNull();
  });

  it("keeps a project joined in another organization off this organization's page", async () => {
    standingIn("user_owner", OWNED, [
      { projectId: "project_elsewhere", organizationId: "org_other", role: Role.Admin },
    ]);

    render(
      await OrganizationProjectsPage(),
    );

    expect(screen.queryByText("project_elsewhere")).not.toBeInTheDocument();
    expect(screen.queryByTestId("projects-shared-with-you")).toBeNull();
    expect(
      screen.getAllByRole("link", { name: /open/i }).map((l) => l.getAttribute("href")),
    ).toEqual(["/project_own_1"]);
  });

  it("lists a project held through the organization role, with nothing to leave", async () => {
    standingIn("user_admin", founders("admin"));
    mockProjects = [{ id: "project_org_wide", name: "Org Wide", role: Role.Admin }];

    render(
      await OrganizationProjectsPage(),
    );

    expect(screen.queryByTestId("projects-shared-with-you")).toBeNull();
    expect(screen.getByText("Org Wide")).toBeInTheDocument();
    expect(screen.getByText("Admin")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /leave/i })).not.toBeInTheDocument();
  });
});
