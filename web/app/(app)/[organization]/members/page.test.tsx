// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { IdentityContext } from "@/lib/server/entities/identity-context";
import { OrganizationRole } from "@/lib/types/organization-role";

// The actions, not their effects: which organization the page hands its
// controls is the assertion, and each action refuses any other.
const actions = vi.hoisted(() => {
  const ok = async () => ({ error: null });
  return {
    cancelOwnershipOfferAction: vi.fn(ok),
    inviteOrganizationMemberAction: vi.fn(ok),
    offerOwnershipAction: vi.fn(ok),
    revokeOrganizationInviteAction: vi.fn(ok),
    removeOrganizationMemberAction: vi.fn(ok),
    updateOrganizationMemberRoleAction: vi.fn(ok),
  };
});
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

const router = vi.hoisted(() => ({ push: vi.fn(), refresh: () => {} }));
vi.mock("next/navigation", () => ({
  useRouter: () => router,
}));

let mockContext: unknown;
let mockOrganizationMembers: unknown;
let mockOrganizationInvites: unknown;
let mockIdentityContext: IdentityContext | null = null;

// Counted, so "refused before it asked" can be asserted rather than implied —
// the gate exists to keep a member's page load off auth entirely, and a test
// that only checks the rendering would pass with the fetch still happening.
let memberFetches = 0;

// The real `activeOrganization`: whose owner a refused member is sent to is
// the assertion, and it has to be the organization the page stands in.
vi.mock("@/lib/server/data", async () => {
  const { activeOrganization } = await vi.importActual<
    typeof import("@/lib/server/entities/organization")
  >("@/lib/server/entities/organization");
  return {
    activeOrganization,
    getServerContext: async () => mockContext,
    fetchOrganizationMembers: async () => {
      memberFetches += 1;
      return mockOrganizationMembers;
    },
    fetchOrganizationInvites: async () => mockOrganizationInvites,
    identityContext: async () => mockIdentityContext,
  };
});

vi.mock("@/lib/server/session", () => ({
  getServerSession: async () => ({
    userId: "user_owner",
    email: "owner@example.com",
    firstName: "Owner",
    lastName: null,
    sessionRowId: null,
  }),
}));

import OrganizationMembersPage from "./page";

// The caller, standing in the owner's organization as `role`. What the page
// shows turns on that role alone — never on whether an id matches theirs.
function standingAs(
  role: IdentityContext["role"],
  userId: string,
  incomingInvites: unknown[] = [],
  name: string | null = "Acme",
) {
  mockIdentityContext = {
    userId,
    organizationId: "org_acme",
    role,
    accessToken: "at_1",
  };
  mockContext = {
    organizations: [
      { organizationId: "org_acme", slug: "acme", name, ownerEmail: "owner@example.com", role },
    ],
    activeOrganizationId: "org_acme",
    memberships: [],
    incomingInvites,
  };
}

const ADMIN = {
  id: "am_2",
  member_id: "user_admin",
  email: "admin@example.com",
  display_name: "Admin User",
  role: OrganizationRole.Admin,
  created_at: "2026-08-15T00:00:00Z",
  is_owner: false,
};

beforeEach(() => {
  vi.clearAllMocks();
  memberFetches = 0;
  standingAs("owner", "user_owner");

  mockOrganizationMembers = {
    kind: "ok",
    members: [
      {
        id: "am_1",
        member_id: "user_owner",
        email: "owner@example.com",
        display_name: "Owner User",
        role: OrganizationRole.Owner,
        created_at: "2026-08-01T00:00:00Z",
        is_owner: true,
      },
      ADMIN,
      {
        id: "am_3",
        member_id: "user_member",
        email: "member@example.com",
        display_name: null,
        role: OrganizationRole.Member,
        created_at: "2026-09-01T00:00:00Z",
        is_owner: false,
      },
    ],
  };
  mockOrganizationInvites = {
    kind: "ok",
    invites: [
      {
        id: "ai_1",
        email: "invited@example.com",
        role: OrganizationRole.Admin,
        expires_at: "2026-09-20T00:00:00Z",
        created_at: "2026-09-12T00:00:00Z",
      },
    ],
  };
});

describe("OrganizationMembersPage", () => {
  it("renders organization role definitions for Owner, Admin, and Member", async () => {
    render(
      await OrganizationMembersPage(),
    );
    expect(screen.getByTestId("page-header")).toHaveTextContent("Members");
    expect(screen.getByText("Full access")).toBeInTheDocument();
    expect(screen.getByText("Can manage")).toBeInTheDocument();
    expect(screen.getByText("Project scoped")).toBeInTheDocument();
  });

  it("renders organization members with their respective roles", async () => {
    render(
      await OrganizationMembersPage(),
    );
    expect(screen.getByText("Organization Owner")).toBeInTheDocument();
    expect(screen.getByText("owner@example.com")).toBeInTheDocument();
    expect(screen.getByText("admin@example.com")).toBeInTheDocument();
    expect(screen.getByText("member@example.com")).toBeInTheDocument();
  });

  it("renders outstanding organization invitations", async () => {
    render(
      await OrganizationMembersPage(),
    );
    expect(screen.getByText("Invited")).toBeInTheDocument();
    expect(screen.getByText("invited@example.com")).toBeInTheDocument();
  });

  it("shows invite action for owner caller", async () => {
    render(
      await OrganizationMembersPage(),
    );
    expect(screen.getByTestId("header-action")).toBeInTheDocument();
  });

  // ⚠ This named "non-owner" and supplied a context with no memberships at
  // all, so the caller had no organization role and the page is now refused
  // outright — the assertions would have passed over an access-denied screen
  // without exercising anything. An ADMIN is the real non-owner case: they may
  // READ the roster and may not manage it.
  it("shows invite action and manage controls for an admin caller, but no ownership transfer", async () => {
    standingAs("admin", "user_admin");
    render(
      await OrganizationMembersPage(),
    );
    expect(screen.getByTestId("page-header")).toBeInTheDocument();
    expect(screen.getByTestId("header-action")).toBeInTheDocument();
    expect(screen.getAllByRole("combobox").length).toBeGreaterThan(0);
    expect(screen.getAllByRole("button", { name: /^remove/i }).length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: /^withdraw/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^transfer ownership/i })).toBeNull();
  });

  // `can_view_org_members` is Owner|Admin, so auth answers a member 403. The
  // page refuses before it asks, rather than rendering its header and its role
  // cards around a restricted panel.
  it("refuses the page to a member, and names who to ask", async () => {
    standingAs("member", "user_member");
    render(await OrganizationMembersPage());
    expect(screen.getByTestId("access-denied")).toBeInTheDocument();
    // The owner by address, since the fixture gives them no name: the one
    // person who can seat a member higher.
    expect(document.body.textContent).toMatch(
      /contact owner@example\.com, the organization.s owner, to request access/i,
    );
    expect(screen.queryByTestId("page-header")).toBeNull();
    expect(
      memberFetches,
      "the page asked auth for a roster it already knew the caller could not have",
    ).toBe(0);
  });

  // ⚠ The roster on screen is `org_acme`'s, so a change made from it has to
  // be aimed at `org_acme` — never at whichever organization the request
  // resolves once this page has gone stale.
  it("aims every roster change at the organization it rendered", async () => {
    render(await OrganizationMembersPage());
    const confirm = async (trigger: string, label: string) => {
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: trigger }));
      });
      await act(async () => {
        fireEvent.click(
          within(screen.getByRole("alertdialog")).getByRole("button", { name: label }),
        );
      });
    };

    // The id alone: the address the removed person is told at is the
    // roster's, read in the action, never the page's argument.
    await confirm("Remove member@example.com", "Remove");
    expect(actions.removeOrganizationMemberAction).toHaveBeenCalledWith(
      "org_acme",
      "user_member",
    );

    await confirm("Withdraw the invitation to invited@example.com", "Withdraw");
    expect(actions.revokeOrganizationInviteAction).toHaveBeenCalledWith("org_acme", "ai_1");

    await confirm("Transfer ownership to admin@example.com", "Send offer");
    expect(actions.offerOwnershipAction).toHaveBeenCalledWith("org_acme", "user_admin");
  });

  it("renders outage when organization members are unavailable", async () => {
    mockOrganizationMembers = { kind: "unavailable" };
    render(
      await OrganizationMembersPage(),
    );
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });

  it("does not render incoming pending invitations on organization members page", async () => {
    standingAs("owner", "user_owner", [
      {
        id: "inc_1",
        scope: "organization",
        targetId: "org_beta",
        targetName: "Beta Labs",
        role: "admin",
        inviterEmail: "founder@example.com",
        inviterDisplayName: "Founder",
        createdAt: "2026-09-12T00:00:00Z",
        expiresAt: "2026-09-19T00:00:00Z",
      },
    ]);
    render(
      await OrganizationMembersPage(),
    );
    expect(screen.queryByTestId("pending-invitations")).toBeNull();
  });
});

// Only the owner hands the organization over, and only to one of its admins —
// a member would take its roster and its deletion with no organization-wide
// powers to use them by.
describe("OrganizationMembersPage: handing the organization over", () => {
  it("offers the owner a hand-over to an admin, and to nobody else", async () => {
    render(await OrganizationMembersPage());
    expect(
      screen.getByRole("button", { name: "Transfer ownership to admin@example.com" }),
    ).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^transfer ownership/i })).toHaveLength(1);
  });

  // An owner holds no project seat, so as an admin they are an admin in every
  // project; the dialog says so before the offer goes.
  it("tells the owner what they keep before the offer goes", async () => {
    render(await OrganizationMembersPage());
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Transfer ownership to admin@example.com" }),
      );
    });
    expect(screen.getByRole("alertdialog")).toHaveTextContent(/an admin of the organization and of every project/i);
  });

  // Auth refuses to offer an unnamed organization: the offer calls it by a name
  // it does not have. The owner is sent to name it, not to a refusal.
  it("sends the owner of an unnamed organization to name it first", async () => {
    standingAs("owner", "user_owner", [], null);
    render(await OrganizationMembersPage());
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Transfer ownership to admin@example.com" }),
      );
    });
    const dialog = screen.getByRole("alertdialog");
    expect(dialog).toHaveTextContent(/name this organization first/i);
    await act(async () => {
      fireEvent.click(within(dialog).getByRole("button", { name: "Go to settings" }));
    });
    expect(router.push).toHaveBeenCalledWith("/acme/settings");
    expect(actions.offerOwnershipAction).not.toHaveBeenCalled();
  });

  it("shows a live offer, and a way to withdraw it rather than make a second", async () => {
    mockOrganizationMembers = {
      kind: "ok",
      members: [{ ...ADMIN, ownership_offer_expires_at: "2026-09-30T00:00:00Z" }],
    };
    render(await OrganizationMembersPage());
    expect(document.body.textContent).toMatch(/ownership offered/i);
    expect(
      screen.getByRole("button", {
        name: "Withdraw the ownership offer to admin@example.com",
      }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^transfer ownership/i })).toBeNull();
  });
});
