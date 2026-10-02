// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { IdentityContext } from "@/lib/server/entities/identity-context";
import type { OrganizationEntry } from "@/lib/server/entities/organization";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => (
    <div data-testid="page-header">{title}</div>
  ),
}));

vi.mock("./_delete-organization", () => ({
  DeleteOrganizationForm: ({
    organizationId,
    organization,
  }: {
    organizationId: string;
    organization: string;
  }) => (
    <div
      data-testid="delete-form"
      data-organization-id={organizationId}
      data-organization={organization}
    />
  ),
}));
vi.mock("./_rename-organization", () => ({
  RenameOrganizationForm: ({
    organizationId,
    initialName,
    fallbackLabel,
    canEdit,
  }: {
    organizationId: string;
    initialName: string;
    fallbackLabel: string;
    canEdit: boolean;
  }) => (
    <div
      data-testid="rename-form"
      data-organization-id={organizationId}
      data-initial={initialName}
      data-fallback={fallbackLabel}
      data-can-edit={String(canEdit)}
    />
  ),
}));

vi.mock("next/navigation", () => ({
  notFound: () => {
    throw new Error("notFound");
  },
  useRouter: () => ({ refresh: () => {} }),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

let mockOrganizations: OrganizationEntry[] = [];
let mockActiveOrganizationId = "org_mine";
let mockIdentityContext: IdentityContext | null = null;
let mockContextDown = false;

// The real `activeOrganization`: which organization the page names is the
// assertion, and it has to be the one its actions will send.
vi.mock("@/lib/server/data", async () => {
  const { activeOrganization } = await vi.importActual<
    typeof import("@/lib/server/entities/organization")
  >("@/lib/server/entities/organization");
  return {
    activeOrganization,
    getServerContext: async () =>
      mockContextDown
        ? null
        : {
            organizations: mockOrganizations,
            activeOrganizationId: mockActiveOrganizationId,
          },
    identityContext: async () => mockIdentityContext,
  };
});

vi.mock("@/lib/server/session", () => ({
  getServerSession: async () => ({
    userId: "user_1",
    email: "k@example.com",
    firstName: "K",
    lastName: null,
    sessionRowId: null,
  }),
}));

import OrganizationSettingsPage from "./page";

const MINE: OrganizationEntry = {
  organizationId: "org_mine",
  slug: "mine",
  name: null,
  ownerEmail: "ada@example.com",
  ownerDisplayName: null,
  role: "owner",
};

// `/me` answers the page and `identityContext` from one resolution, so the
// two always agree on where the caller stands and as what.
function standIn(organizations: OrganizationEntry[], organizationId: string) {
  mockOrganizations = organizations;
  mockActiveOrganizationId = organizationId;
  const active = organizations.find((o) => o.organizationId === organizationId)!;
  mockIdentityContext = {
    userId: "user_1",
    organizationId,
    role: active.role,
    accessToken: "at_1",
  };
}

const headings = () =>
  screen
    .getAllByRole("heading", { level: 2 })
    .map((h) => h.textContent?.toLowerCase() ?? "");

beforeEach(() => {
  mockContextDown = false;
  standIn([MINE], "org_mine");
});

describe("OrganizationSettingsPage", () => {
  // ⚠ An outage at `/me` is not a refusal. It used to render "Access denied"
  // to the owner; every sibling page says the service is down, and so does
  // this one.
  it("says the service is down when `/me` cannot be read, not that access is denied", async () => {
    mockContextDown = true;
    render(await OrganizationSettingsPage());
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    expect(screen.queryByTestId("access-denied")).toBeNull();
  });

  it("holds the organization's name, and nothing of the person's", async () => {
    render(await OrganizationSettingsPage());
    expect(screen.getByTestId("page-header")).toHaveTextContent("Settings");
    expect(screen.getByTestId("rename-form")).toBeInTheDocument();
    expect(screen.queryByText("k@example.com")).toBeNull();
    expect(screen.queryByTestId("sessions")).toBeNull();
  });

  // ⚠ This page offered no way to destroy anything while deleting an
  // organization meant deleting its owner's account, which lives on Privacy.
  // The two are separate now: the organization goes from here, its owner's
  // account stays, and the danger zone is the OWNER's alone.
  it("ends with the danger zone for the owner, and it deletes the organization", async () => {
    render(await OrganizationSettingsPage());
    expect(headings()).toEqual(["name", "danger zone"]);
    expect(screen.getByTestId("delete-form")).toHaveAttribute(
      "data-organization",
      "ada@example.com",
    );
    expect(document.body.textContent).toMatch(/your account stays/i);
  });

  it("offers an admin no way to destroy anything, but allows rename", async () => {
    standIn([{ ...MINE, role: "admin" }], "org_mine");
    render(await OrganizationSettingsPage());
    expect(headings()).toEqual(["name"]);
    expect(screen.queryByText(/danger zone/i)).toBeNull();
    expect(screen.queryByTestId("delete-form")).toBeNull();
    expect(screen.getByTestId("rename-form")).toHaveAttribute("data-can-edit", "true");
  });

  it("offers the rename to the owner", async () => {
    render(await OrganizationSettingsPage());
    expect(screen.getByTestId("rename-form")).toHaveAttribute("data-can-edit", "true");
  });

  // Each action refuses an organization other than the one it is handed, so
  // what it is handed has to be the one this page names — the check is only
  // as good as the id on this side of it.
  it("hands both forms the id of the organization it names", async () => {
    render(await OrganizationSettingsPage());
    expect(screen.getByTestId("rename-form")).toHaveAttribute(
      "data-organization-id",
      "org_mine",
    );
    expect(screen.getByTestId("delete-form")).toHaveAttribute(
      "data-organization-id",
      "org_mine",
    );
  });

  // ⚠ **THE BUG THIS PAGE SHIPPED WITH.** It read `/me` — always the signed-in
  // person's OWN organization — while `renameOrganizationAction` sends the
  // ACTIVE one upstream. Switched into somebody else's organization, the box
  // showed your organization's name and Save renamed theirs. Auth refused it
  // below owner, so it usually read as an unexplainable 403; an owner of two
  // organizations would have renamed the wrong one to a name they never typed.
  //
  // The test above named a member case it never exercised, which is how this
  // got through. These do exercise it.
  describe("standing in somebody else's organization", () => {
    const THEIRS: OrganizationEntry = {
      organizationId: "org_theirs",
      slug: "theirs",
      name: null,
      ownerEmail: "owner@example.com",
      ownerDisplayName: "Grace Hopper",
      // An ADMIN, so the read-only view still renders and the assertions
      // below stay about what they were always about — which organization the
      // page names. A member is refused outright; that is the test after these.
      role: "admin",
    };

    beforeEach(() => {
      standIn([{ ...MINE, name: "My Own Workspace" }, THEIRS], "org_theirs");
    });

    // Every organization's entry carries its own name now, so the form can
    // show the one you are in — editable by owner and admin. The rule
    // that survives is the one this test was for: never another's name.
    it("shows its name and allows rename, and never prefills your own", async () => {
      render(await OrganizationSettingsPage());
      const form = screen.getByTestId("rename-form");
      expect(form).toHaveAttribute("data-can-edit", "true");
      expect(form).toHaveAttribute("data-initial", "");
      expect(
        document.body.innerHTML,
        "your own organization's name is on another organization's settings page",
      ).not.toContain("My Own Workspace");
    });

    it("names the organization you are actually in, by its owner's address", async () => {
      render(await OrganizationSettingsPage());
      // The address, not the owner's name: this page is about a workspace.
      const form = screen.getByTestId("rename-form");
      expect(form).toHaveAttribute("data-fallback", "owner@example.com");
      expect(form).toHaveAttribute("data-organization-id", "org_theirs");
      expect(document.body.innerHTML).not.toContain("Grace Hopper");
    });

    it("offers no danger zone in an organization you do not own", async () => {
      render(await OrganizationSettingsPage());
      expect(screen.queryByTestId("delete-form")).toBeNull();
      expect(screen.queryByText(/danger zone/i)).toBeNull();
    });

    // ⚠ A member holds no organization-wide powers, so organization settings
    // are not theirs to read either. Auth refuses the rename below owner; this
    // is the console refusing the page before anybody presses anything.
    it("refuses the page outright to a member", async () => {
      standIn([{ ...MINE, name: "My Own Workspace" }, { ...THEIRS, role: "member" }], "org_theirs");
      render(await OrganizationSettingsPage());
      expect(screen.getByTestId("access-denied")).toBeInTheDocument();
      expect(document.body.textContent).toMatch(
        /contact grace hopper, the organization.s owner, to request access/i,
      );
      expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
        "href",
        "/theirs",
      );
      expect(screen.queryByTestId("rename-form")).toBeNull();
    });

    it("still offers the rename once you switch back", async () => {
      standIn([{ ...MINE, name: "My Own Workspace" }, THEIRS], "org_mine");
      render(await OrganizationSettingsPage());
      expect(screen.getByTestId("rename-form")).toHaveAttribute(
        "data-initial",
        "My Own Workspace",
      );
    });
  });

  // ⚠ The rename used to exist only for "your own organization" — the one
  // whose id was your user id — because `/me` carried no other organization's
  // name. Any organization you own is yours to name and to delete: a second
  // one, or one handed to you by its previous owner.
  it("offers the rename and the danger zone in any organization the caller owns", async () => {
    standIn(
      [
        { ...MINE, name: "My Own Workspace" },
        {
          organizationId: "org_handed_over",
          slug: "second-shop",
          name: "Second Shop",
          ownerEmail: "k@example.com",
          role: "owner",
        },
      ],
      "org_handed_over",
    );
    render(await OrganizationSettingsPage());
    const form = screen.getByTestId("rename-form");
    expect(form).toHaveAttribute("data-can-edit", "true");
    expect(form).toHaveAttribute("data-initial", "Second Shop");
    expect(form).toHaveAttribute("data-organization-id", "org_handed_over");
    const deletion = screen.getByTestId("delete-form");
    expect(deletion).toHaveAttribute("data-organization", "Second Shop");
    expect(deletion).toHaveAttribute("data-organization-id", "org_handed_over");
    expect(document.body.innerHTML).not.toContain("My Own Workspace");
  });

  it("hands the form the current name, and what to show while there is none", async () => {
    standIn([{ ...MINE, name: "Acme Robotics" }], "org_mine");
    render(await OrganizationSettingsPage());
    const form = screen.getByTestId("rename-form");
    expect(form).toHaveAttribute("data-initial", "Acme Robotics");
  });

  // ⚠ **The ADDRESS, which is what the rail and the selector actually print
  // for an unnamed organization.** It was the word "Personal", so the
  // placeholder named a label no other surface would show.
  it("shows the address as the placeholder when nobody has renamed it", async () => {
    render(await OrganizationSettingsPage());
    const form = screen.getByTestId("rename-form");
    expect(form).toHaveAttribute("data-initial", "");
    expect(form).toHaveAttribute("data-fallback", "ada@example.com");
  });
});
