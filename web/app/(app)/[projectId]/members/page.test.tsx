// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { Role } from "@/lib/types/enums";

const actions = vi.hoisted(() => ({
  inviteMemberAction: vi.fn(async () => ({ error: null })),
  removeMemberAction: vi.fn(async () => ({ error: null })),
  revokeInviteAction: vi.fn(async () => ({ error: null })),
  updateMemberRoleAction: vi.fn(async () => ({ error: null })),
  offerProjectAction: vi.fn(async () => ({ error: null })),
  cancelProjectOfferAction: vi.fn(async () => ({ error: null })),
}));
vi.mock("./actions", () => actions);

vi.mock("@/components/page-header", () => ({
  PageHeader: ({
    title,
    action,
  }: {
    title: string;
    action?: React.ReactNode;
  }) => (
    <div data-testid="page-header">
      <span>{title}</span>
      {action && <div data-testid="header-action">{action}</div>}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

vi.mock("@/components/role-restricted", () => ({
  RoleRestricted: ({ what }: { what: string }) => (
    <div data-testid="role-restricted">{what}</div>
  ),
}));

// The route's project, which is what the controls aim every action at; the
// page below renders the same one.
vi.mock("next/navigation", () => ({
  useParams: () => ({ projectId: "project_7bQx2mNv9BcK4dLp" }),
  useRouter: () => ({ refresh: () => {} }),
}));

let mockContext: unknown;
let mockMembers: unknown;
let mockInvites: unknown;
let mockProjects: unknown;

vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => mockContext,
  fetchMembers: async () => mockMembers,
  fetchInvites: async () => mockInvites,
  fetchProjects: async () => mockProjects,
}));

vi.mock("@/lib/server/session", () => ({
  getServerSession: async () => ({
    userId: "user_owner",
    email: "owner@example.com",
    firstName: "Owner",
    lastName: null,
    sessionRowId: null,
  }),
}));

import MembersPage from "./page";

// `/me` for the owner of the organization the project is in.
const ownerContext = (incomingInvites: unknown[]) => ({
  person: { userId: "user_owner", email: "owner@example.com", analyticsOptIn: false },
  organizations: [
    { organizationId: "org_owner", name: null, ownerEmail: "owner@example.com", role: "owner" },
  ],
  activeOrganizationId: "org_owner",
  memberships: [],
  incomingInvites,
  flags: {},
});

beforeEach(() => {
  mockContext = ownerContext([]);
  mockMembers = {
    kind: "ok",
    members: [
      {
        id: "m_owner",
        member_id: "user_owner",
        email: "owner@example.com",
        display_name: "Owner User",
        role: Role.Owner,
        created_at: "2026-08-01T00:00:00Z",
        is_owner: true,
      },
      {
        id: "m_1",
        member_id: "user_2",
        email: "admin@example.com",
        display_name: "Admin User",
        role: Role.Admin,
        created_at: "2026-09-01T00:00:00Z",
        is_owner: false,
      },
    ],
  };
  mockInvites = { kind: "ok", invites: [] };
  mockProjects = [];

});

describe("MembersPage", () => {

  describe("project mode", () => {
    it("renders the project role definitions for Owner, Admin, and Member", async () => {
      render(
        await MembersPage({
          params: Promise.resolve({ projectId: "project_7bQx2mNv9BcK4dLp" }),
        }),
      );
      expect(screen.getByText("Full access")).toBeInTheDocument();
      expect(screen.getByText("Can manage")).toBeInTheDocument();
      expect(screen.getByText("Read-only")).toBeInTheDocument();
    });

    it("renders project members roster", async () => {
      mockProjects = [
        {
          id: "project_7bQx2mNv9BcK4dLp",
          name: "My Project",
          role: Role.Owner,
        },
      ];
      render(
        await MembersPage({
          params: Promise.resolve({ projectId: "project_7bQx2mNv9BcK4dLp" }),
        }),
      );
      expect(screen.getByTestId("page-header")).toHaveTextContent("Members");
      expect(screen.getByText("Owner User")).toBeInTheDocument();
      expect(screen.getByText("Project Owner")).toBeInTheDocument();
      expect(screen.getByText("admin@example.com")).toBeInTheDocument();
      expect(screen.getByTestId("header-action")).toBeInTheDocument();
      expect(screen.getByRole("combobox")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: /^remove/i })).toBeInTheDocument();
    });

    it("shows invite action and manage controls when caller is project admin", async () => {
      mockProjects = [
        {
          id: "project_7bQx2mNv9BcK4dLp",
          name: "My Project",
          role: Role.Admin,
        },
      ];
      render(
        await MembersPage({
          params: Promise.resolve({ projectId: "project_7bQx2mNv9BcK4dLp" }),
        }),
      );
      expect(screen.getByTestId("page-header")).toHaveTextContent("Members");
      expect(screen.getByTestId("header-action")).toBeInTheDocument();
      expect(screen.getByRole("combobox")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: /^remove/i })).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: /^transfer project/i })).toBeNull();
    });

    it("hides invite action and manage controls when caller is member", async () => {
      mockProjects = [
        {
          id: "project_7bQx2mNv9BcK4dLp",
          name: "My Project",
          role: Role.Member,
        },
      ];
      render(
        await MembersPage({
          params: Promise.resolve({ projectId: "project_7bQx2mNv9BcK4dLp" }),
        }),
      );
      expect(screen.getByTestId("page-header")).toHaveTextContent("Members");
      expect(screen.getByText("admin@example.com")).toBeInTheDocument();
      expect(screen.queryByTestId("header-action")).toBeNull();
      expect(screen.queryByRole("combobox")).toBeNull();
      expect(screen.queryByRole("button", { name: /^remove/i })).toBeNull();
    });

    it("does not render incoming invitations on project members page", async () => {
      mockContext = ownerContext([
        {
          id: "inc_2",
          scope: "project",
          targetId: "project_prod",
          targetName: "Prod Infra",
          role: "admin",
          inviterEmail: "lead@example.com",
          inviterDisplayName: "Project Lead",
          createdAt: "2026-09-12T00:00:00Z",
          expiresAt: "2026-09-19T00:00:00Z",
        },
      ]);
      render(
        await MembersPage({
          params: Promise.resolve({ projectId: "project_7bQx2mNv9BcK4dLp" }),
        }),
      );
      expect(screen.queryByTestId("pending-invitations")).toBeNull();
    });
  });
});

// Only the owner hands the project over, and only to one of its admins — a
// member would take its keys with no say over its members to go with them.
describe("MembersPage: handing the project over", () => {
  const PROJECT = "project_7bQx2mNv9BcK4dLp";
  const page = async () => MembersPage({ params: Promise.resolve({ projectId: PROJECT }) });

  beforeEach(() => {
    vi.clearAllMocks();
    mockProjects = [{ id: PROJECT, name: "Payments", role: Role.Owner }];
    mockMembers = {
      kind: "ok",
      members: [
        {
          id: "m_owner",
          member_id: "user_owner",
          email: "owner@example.com",
          display_name: "Owner User",
          role: Role.Owner,
          created_at: "2026-08-01T00:00:00Z",
          is_owner: true,
        },
        {
          id: "m_1",
          member_id: "user_2",
          email: "admin@example.com",
          display_name: "Admin User",
          role: Role.Admin,
          created_at: "2026-09-01T00:00:00Z",
          is_owner: false,
        },
        {
          id: "m_2",
          member_id: "user_3",
          email: "member@example.com",
          display_name: null,
          role: Role.Member,
          created_at: "2026-09-02T00:00:00Z",
          is_owner: false,
        },
      ],
    };
  });

  it("offers the owner a hand-over to an admin, and to nobody else", async () => {
    render(await page());
    expect(
      screen.getByRole("button", { name: "Transfer project to admin@example.com" }),
    ).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^transfer project/i })).toHaveLength(1);
  });

  // ⚠ The offer is confirmed first, and it says what comes with the project
  // and what stays behind — the keys travel, the connectors do not.
  it("sends the offer for the project on the page, once confirmed", async () => {
    render(await page());
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Transfer project to admin@example.com" }),
      );
    });
    const dialog = screen.getByRole("alertdialog");
    expect(dialog).toHaveTextContent(/connectors stay with your organization/i);
    expect(actions.offerProjectAction, "offered before it was confirmed").not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.click(within(dialog).getByRole("button", { name: "Send offer" }));
    });
    expect(actions.offerProjectAction).toHaveBeenCalledWith(PROJECT, "user_2");
  });

  it("shows a live offer, and a way to withdraw it rather than make a second", async () => {
    mockMembers = {
      kind: "ok",
      members: [
        {
          id: "m_1",
          member_id: "user_2",
          email: "admin@example.com",
          display_name: "Admin User",
          role: Role.Admin,
          created_at: "2026-09-01T00:00:00Z",
          is_owner: false,
          transfer_offer_expires_at: "2026-10-05T00:00:00Z",
        },
      ],
    };
    render(await page());
    expect(screen.getByText(/project offered/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^transfer project/i })).toBeNull();
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", {
          name: "Withdraw the project offer to admin@example.com",
        }),
      );
    });
    await act(async () => {
      fireEvent.click(
        within(screen.getByRole("alertdialog")).getByRole("button", { name: "Withdraw" }),
      );
    });
    expect(actions.cancelProjectOfferAction).toHaveBeenCalledWith(PROJECT);
  });

  it("offers nothing to somebody who is not the owner", async () => {
    mockProjects = [{ id: PROJECT, name: "Payments", role: Role.Admin }];
    render(await page());
    expect(screen.queryByRole("button", { name: /^transfer project/i })).toBeNull();
  });
});
