// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { Role } from "@/lib/types/enums";

const actions = vi.hoisted(() => ({
  addMemberAction: vi.fn(async (): Promise<{ error: string | null }> => ({ error: null })),
  inviteMemberAction: vi.fn(async () => ({ error: null })),
  leaveProjectAction: vi.fn(async (): Promise<{ error: string | null }> => ({ error: null })),
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

vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh: () => {} }),
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }),
}));

// Radix's listbox opens on pointer events jsdom does not dispatch; a native
// select stands in, with the same value in and the same value out.
vi.mock("@/components/ui/select", () => ({
  Select: ({
    value,
    onValueChange,
    disabled,
    children,
  }: {
    value: string;
    onValueChange: (v: string) => void;
    disabled?: boolean;
    children: React.ReactNode;
  }) => (
    <select value={value} disabled={disabled} onChange={(e) => onValueChange(e.target.value)}>
      {children}
    </select>
  ),
  SelectTrigger: () => null,
  SelectValue: () => null,
  SelectContent: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  SelectItem: ({ value, children }: { value: string; children: React.ReactNode }) => (
    <option value={value}>{children}</option>
  ),
}));

let mockContext: unknown;
let mockMembers: unknown;
let mockInvites: unknown;
let mockProject: unknown;
// The organization the page stands in, with the caller's role there: `null`
// leaves the roster unread, as a page outside any organization would.
let mockHome: { role: string } | null = null;
let mockRoster: unknown = { kind: "ok", members: [] };
const fetchMembers = vi.fn<(projectId: string) => Promise<unknown>>(async () => mockMembers);
const fetchInvites = vi.fn<(projectId: string) => Promise<unknown>>(async () => mockInvites);
const fetchOrganizationMembers = vi.fn(async () => mockRoster);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => mockHome,
  getServerContext: async () => mockContext,
  fetchMembers: (projectId: string) => fetchMembers(projectId),
  fetchInvites: (projectId: string) => fetchInvites(projectId),
  fetchOrganizationMembers: () => fetchOrganizationMembers(),
  fetchProjectBySlug: async () => mockProject,
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

import { toast } from "sonner";

import MembersPage from "./page";

// The path spells the project with slugs. The page finds it by them, and
// aims the roster's reads and every control at its id.
const PROJECT = "project_7bQx2mNv9BcK4dLp";
const PARAMS = Promise.resolve({ organization: "acme", project: "my-project" });
const projectAs = (role: Role, name = "My Project") => ({
  id: PROJECT,
  slug: "my-project",
  name,
  role,
});

// `/me` for the owner of the organization the project is in.
const ownerContext = (incomingInvites: unknown[]) => ({
  person: { userId: "user_owner", email: "owner@example.com", analyticsOptIn: false },
  organizations: [
    {
      organizationId: "org_owner",
      slug: "acme",
      name: "Acme",
      ownerEmail: "owner@example.com",
      role: "owner",
    },
  ],
  activeOrganizationId: "org_owner",
  memberships: [],
  incomingInvites,
  flags: {},
});

beforeEach(() => {
  vi.clearAllMocks();
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
  mockProject = projectAs(Role.Member);
  mockHome = null;
  mockRoster = { kind: "ok", members: [] };
});

// The organization's roster, as its owner reads it: the owner, the admin
// already seated on the project, Ada, an admin of the organization who holds
// every project already, and Dana, a member holding no seat here.
const ROSTER = {
  kind: "ok",
  members: [
    { id: "om_owner", member_id: "user_owner", email: "owner@example.com", display_name: "Owner User", role: "owner", created_at: "2026-08-01T00:00:00Z", is_owner: true },
    { id: "om_2", member_id: "user_2", email: "admin@example.com", display_name: "Admin User", role: "member", created_at: "2026-09-01T00:00:00Z", is_owner: false },
    { id: "om_ada", member_id: "user_ada", email: "ada@example.com", display_name: "Ada", role: "admin", created_at: "2026-09-01T00:00:00Z", is_owner: false },
    { id: "om_3", member_id: "user_dana", email: "dana@example.com", display_name: "Dana", role: "member", created_at: "2026-09-02T00:00:00Z", is_owner: false },
  ],
};

const press = async (name: RegExp | string) => {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name }));
  });
};

// A member of the organization is seated with nothing to accept, from the
// roster only its owner and admins read; everybody else is invited by email.
describe("MembersPage: adding from the organization", () => {
  const asOwner = async (roster: unknown = ROSTER) => {
    mockProject = projectAs(Role.Owner);
    mockHome = { role: "owner" };
    mockRoster = roster;
    render(await MembersPage({ params: PARAMS }));
    await press(/^Add member/);
    return screen.getByRole("dialog");
  };
  const choose = async (dialog: HTMLElement, memberId: string) => {
    const [people] = within(dialog).getAllByRole("combobox");
    await act(async () => {
      fireEvent.change(people!, { target: { value: memberId } });
    });
  };

  it("offers the owner the members of the organization not yet seated here, and seats the one chosen", async () => {
    const dialog = await asOwner();
    expect(fetchOrganizationMembers).toHaveBeenCalled();
    expect(within(dialog).getByRole("radio", { name: "From the organization" })).toHaveAttribute("aria-checked", "true");
    const [people] = within(dialog).getAllByRole("combobox");
    const names = within(people!).getAllByRole("option").map((o) => o.textContent);
    expect(names).toEqual(["Dana — dana@example.com"]);

    // Nobody is chosen for the owner: a seat given mails its person.
    expect(within(dialog).getByRole("button", { name: "Add" })).toBeDisabled();
    await choose(dialog, "user_dana");
    await press("Add");
    expect(actions.addMemberAction).toHaveBeenCalledWith(PROJECT, "user_dana", "member");
    expect(actions.inviteMemberAction).not.toHaveBeenCalled();
    expect(toast.success).toHaveBeenCalledWith("Added Dana as a member.");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("seats at the role chosen", async () => {
    const dialog = await asOwner();
    await choose(dialog, "user_dana");
    const [, role] = within(dialog).getAllByRole("combobox");
    await act(async () => {
      fireEvent.change(role!, { target: { value: "admin" } });
    });
    await press("Add");
    expect(actions.addMemberAction).toHaveBeenCalledWith(PROJECT, "user_dana", "admin");
    expect(toast.success).toHaveBeenCalledWith("Added Dana as an admin.");
  });

  it("keeps the dialog open with auth's refusal", async () => {
    actions.addMemberAction.mockResolvedValueOnce({ error: "that person is already a member" });
    const dialog = await asOwner();
    await choose(dialog, "user_dana");
    await press("Add");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(within(screen.getByRole("dialog")).getByRole("alert")).toHaveTextContent(
      "that person is already a member",
    );
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("still invites by email, once the way is switched", async () => {
    await asOwner();
    await act(async () => {
      fireEvent.click(screen.getByRole("radio", { name: "By email" }));
    });
    const dialog = screen.getByRole("dialog");
    await act(async () => {
      fireEvent.change(within(dialog).getByLabelText("Email address"), {
        target: { value: "new@example.com" },
      });
    });
    await press("Send invitation");
    expect(actions.inviteMemberAction).toHaveBeenCalledWith(PROJECT, "new@example.com", "member");
    expect(actions.addMemberAction).not.toHaveBeenCalled();
  });

  it("hands a project admin who is only a member of the organization the email form alone", async () => {
    mockProject = projectAs(Role.Admin);
    mockHome = { role: "member" };
    mockRoster = ROSTER;
    render(await MembersPage({ params: PARAMS }));
    expect(fetchOrganizationMembers).not.toHaveBeenCalled();
    await press(/^Add member/);
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).queryByRole("radiogroup")).toBeNull();
    expect(within(dialog).getByLabelText("Email address")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Send invitation" })).toBeDisabled();
    expect(within(dialog).queryByRole("note")).toBeNull();
  });

  it("offers the email form alone when every member of the organization holds a seat here", async () => {
    const dialog = await asOwner({ kind: "ok", members: ROSTER.members.slice(0, 2) });
    expect(within(dialog).queryByRole("radiogroup")).toBeNull();
    expect(within(dialog).getByLabelText("Email address")).toBeInTheDocument();
    expect(within(dialog).queryByRole("note")).toBeNull();
  });

  it("forgets one way's refusal on switching to the other", async () => {
    actions.addMemberAction.mockResolvedValueOnce({ error: "that person is already a member" });
    const dialog = await asOwner();
    await choose(dialog, "user_dana");
    await press("Add");
    expect(within(dialog).getByText("that person is already a member")).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(within(dialog).getByRole("radio", { name: "By email" }));
    });
    expect(within(dialog).queryByText("that person is already a member")).toBeNull();
  });

  // The add's answer is the roster's: the way stays put until it comes.
  it("holds the way while an add is under way", async () => {
    actions.addMemberAction.mockReturnValueOnce(new Promise(() => {}));
    const dialog = await asOwner();
    await choose(dialog, "user_dana");
    await press("Add");
    await act(async () => {
      fireEvent.click(within(dialog).getByRole("radio", { name: "By email" }));
    });
    expect(within(dialog).getByRole("radio", { name: "From the organization" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(within(dialog).getByRole("button", { name: "Adding…" })).toBeDisabled();
  });

  // At the ceiling an invitation would be refused, and a seat from the roster
  // adds nobody: the roster stays offered, and the way by email says why not.
  it("keeps the roster open at the organization's ceiling, the invitation closed with its sentence", async () => {
    mockMembers = {
      ...(mockMembers as object),
      ceiling: { limit: 1, held: 3, detail: "Hobby holds one member." },
    };
    const dialog = await asOwner();
    expect(within(dialog).getByRole("radio", { name: "By email" })).toBeDisabled();
    expect(within(dialog).getByRole("note")).toHaveTextContent("Hobby holds one member.");
    expect(within(dialog).getByRole("radiogroup")).toHaveAccessibleDescription(
      "Hobby holds one member.",
    );
    await choose(dialog, "user_dana");
    await press("Add");
    expect(actions.addMemberAction).toHaveBeenCalledWith(PROJECT, "user_dana", "member");
  });

  it("offers nothing at the ceiling with nobody left to seat", async () => {
    mockProject = projectAs(Role.Owner);
    mockHome = { role: "owner" };
    mockRoster = { kind: "ok", members: ROSTER.members.slice(0, 2) };
    mockMembers = {
      ...(mockMembers as object),
      ceiling: { limit: 1, held: 3, detail: "Hobby holds one member." },
    };
    render(await MembersPage({ params: PARAMS }));
    expect(screen.queryByTestId("header-action")).toBeNull();
    expect(screen.getByTestId("member-ceiling")).toHaveTextContent("Hobby holds one member.");
  });

  it("says so when the organization's members cannot be read, rather than offering nobody in silence", async () => {
    const dialog = await asOwner({ kind: "unavailable" });
    expect(within(dialog).queryByRole("radiogroup")).toBeNull();
    expect(within(dialog).getByRole("note")).toHaveTextContent(/can’t be read right now/);
  });
});

describe("MembersPage", () => {

  describe("project mode", () => {
    it("renders the project role definitions for Owner, Admin, and Member", async () => {
      render(
        await MembersPage({ params: PARAMS }),
      );
      expect(screen.getByText("Full access")).toBeInTheDocument();
      expect(screen.getByText("Can manage")).toBeInTheDocument();
      expect(screen.getByText("Read-only")).toBeInTheDocument();
    });

    it("renders project members roster", async () => {
      mockProject = projectAs(Role.Owner);
      render(
        await MembersPage({ params: PARAMS }),
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
      mockProject = projectAs(Role.Admin);
      render(
        await MembersPage({ params: PARAMS }),
      );
      expect(screen.getByTestId("page-header")).toHaveTextContent("Members");
      expect(screen.getByTestId("header-action")).toBeInTheDocument();
      expect(screen.getByRole("combobox")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: /^remove/i })).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: /^transfer project/i })).toBeNull();
    });

    // A seat here counts against the owning organization's ceiling: at it,
    // the button goes and the deployment's sentence stands where it was, to
    // whoever could have invited; a member, who could not, reads nothing.
    it("hides the invitation at the organization's ceiling, and says why", async () => {
      mockProject = projectAs(Role.Owner);
      mockMembers = {
        ...(mockMembers as object),
        ceiling: { limit: 1, held: 1, detail: "Hobby holds one member." },
      };
      render(await MembersPage({ params: PARAMS }));
      expect(screen.queryByTestId("header-action")).toBeNull();
      expect(screen.getByTestId("member-ceiling")).toHaveTextContent("Hobby holds one member.");
    });

    it("says nothing of the ceiling to a member, who could not invite anyway", async () => {
      mockProject = projectAs(Role.Member);
      mockMembers = {
        ...(mockMembers as object),
        ceiling: { limit: 1, held: 1, detail: "Hobby holds one member." },
      };
      render(await MembersPage({ params: PARAMS }));
      expect(screen.queryByTestId("member-ceiling")).toBeNull();
    });

    it("hides invite action and manage controls when caller is member", async () => {
      mockProject = projectAs(Role.Member);
      render(
        await MembersPage({ params: PARAMS }),
      );
      expect(screen.getByTestId("page-header")).toHaveTextContent("Members");
      expect(screen.getByText("admin@example.com")).toBeInTheDocument();
      expect(screen.queryByTestId("header-action")).toBeNull();
      expect(screen.queryByRole("combobox")).toBeNull();
      expect(screen.queryByRole("button", { name: /^remove/i })).toBeNull();
    });

    it("reads the roster and the invitations by the project's id, not its slug", async () => {
      render(await MembersPage({ params: PARAMS }));
      expect(fetchMembers).toHaveBeenCalledWith(PROJECT);
      expect(fetchInvites).toHaveBeenCalledWith(PROJECT);
    });

    it("says so when the project cannot be read, rather than a roster of nothing", async () => {
      mockProject = null;
      render(await MembersPage({ params: PARAMS }));
      expect(screen.getByTestId("outage")).toBeInTheDocument();
      expect(fetchMembers).not.toHaveBeenCalled();
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
        await MembersPage({ params: PARAMS }),
      );
      expect(screen.queryByTestId("pending-invitations")).toBeNull();
    });
  });
});

// Only the owner hands the project over, and only to one of its admins — a
// member would take its keys with no say over its members to go with them.
describe("MembersPage: handing the project over", () => {
  const page = async () => MembersPage({ params: PARAMS });

  beforeEach(() => {
    vi.clearAllMocks();
    mockProject = projectAs(Role.Owner, "Payments");
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
    mockProject = projectAs(Role.Admin, "Payments");
    render(await page());
    expect(screen.queryByRole("button", { name: /^transfer project/i })).toBeNull();
  });
});

// A seat is its holder's to give up: the caller's own row carries Leave in
// place of Remove, whatever their role and however the seat came, and the
// owner's row carries neither — the project is theirs to delete.
describe("MembersPage: leaving", () => {
  const page = async () => MembersPage({ params: PARAMS });
  // Who is reading the page, as `/me` names them.
  const asPerson = (userId: string) => {
    mockContext = {
      ...ownerContext([]),
      person: { userId, email: "me@example.com", analyticsOptIn: false },
    };
  };
  const ROWS = [
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
      display_name: "Member User",
      role: Role.Member,
      created_at: "2026-09-02T00:00:00Z",
      is_owner: false,
    },
  ];

  beforeEach(() => {
    mockMembers = { kind: "ok", members: ROWS };
  });

  it("offers a member Leave on their own row, and nothing on anybody else's", async () => {
    asPerson("user_3");
    mockProject = projectAs(Role.Member);
    render(await page());
    expect(screen.getByRole("button", { name: "Leave My Project" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^leave/i })).toHaveLength(1);
    expect(screen.queryByRole("button", { name: /^remove/i })).toBeNull();
    expect(screen.queryByRole("combobox")).toBeNull();
  });

  it("gives an admin Leave on their own row and Remove on everybody else's but the owner's", async () => {
    asPerson("user_2");
    mockProject = projectAs(Role.Admin);
    render(await page());
    expect(screen.getByRole("button", { name: "Leave My Project" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove admin@example.com" })).toBeNull();
    expect(screen.getByRole("button", { name: "Remove member@example.com" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^remove/i })).toHaveLength(1);
  });

  it("offers the owner no Leave: the project is theirs to delete", async () => {
    asPerson("user_owner");
    mockProject = projectAs(Role.Owner);
    render(await page());
    expect(screen.queryByRole("button", { name: /^leave/i })).toBeNull();
    expect(screen.getAllByRole("button", { name: /^remove/i })).toHaveLength(2);
  });

  // The seat goes once the dialog is confirmed, and the action is told to
  // lead the person out: the page they stand on is no longer theirs to read.
  it("gives up the seat once confirmed, and is led out of the project", async () => {
    asPerson("user_3");
    mockProject = projectAs(Role.Member);
    render(await page());
    await press("Leave My Project");
    const dialog = screen.getByRole("alertdialog");
    expect(dialog).toHaveTextContent("Leave My Project?");
    expect(dialog).toHaveTextContent(/an owner or admin can add you back/i);
    expect(actions.leaveProjectAction, "left before it was confirmed").not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.click(within(dialog).getByRole("button", { name: "Leave" }));
    });
    expect(actions.leaveProjectAction).toHaveBeenCalledWith(PROJECT, "project");
    expect(actions.removeMemberAction).not.toHaveBeenCalled();
  });

  it("shows auth's refusal on the row", async () => {
    actions.leaveProjectAction.mockResolvedValueOnce({ error: "the project is busy — try again in a moment" });
    asPerson("user_2");
    mockProject = projectAs(Role.Admin);
    render(await page());
    await press("Leave My Project");
    await act(async () => {
      fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Leave" }));
    });
    expect(screen.getByText("the project is busy — try again in a moment")).toBeInTheDocument();
  });
});
