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
vi.mock("./_organization-name", () => ({
  OrganizationNameForm: ({
    organizationId,
    initialName,
    canEdit,
  }: {
    organizationId: string;
    initialName: string;
    canEdit: boolean;
  }) => (
    <div
      data-testid="name-form"
      data-organization-id={organizationId}
      data-initial={initialName}
      data-can-edit={String(canEdit)}
    />
  ),
}));
vi.mock("./_organization-url", () => ({
  OrganizationUrlForm: ({
    organizationId,
    slug,
    host,
    canEdit,
  }: {
    organizationId: string;
    slug: string;
    host: string;
    canEdit: boolean;
  }) => (
    <div
      data-testid="url-form"
      data-organization-id={organizationId}
      data-slug={slug}
      data-host={host}
      data-can-edit={String(canEdit)}
    />
  ),
}));
vi.mock("@/components/settings-row", () => ({
  SettingsRow: ({ label, children }: { label: string; children: React.ReactNode }) => (
    <div data-testid="settings-row" data-label={label}>
      {children}
    </div>
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
  name: "My Own Workspace",
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

  // Vercel's team page, for an organization: the name and the URL are two
  // settings, the id is shown for the API, and the danger zone is the owner's.
  it("shows the name, the URL, the id and the danger zone, in that order, to the owner", async () => {
    render(await OrganizationSettingsPage());
    expect(screen.getByTestId("page-header")).toHaveTextContent("Settings");
    expect(headings()).toEqual(["name", "url", "organization id", "danger zone"]);
    expect(screen.getByTestId("name-form")).toHaveAttribute("data-initial", "My Own Workspace");
    expect(screen.getByTestId("name-form")).toHaveAttribute("data-can-edit", "true");
    expect(screen.getByTestId("url-form")).toHaveAttribute("data-slug", "mine");
    expect(screen.getByTestId("url-form")).toHaveAttribute("data-host", "example.com");
    expect(screen.getByTestId("url-form")).toHaveAttribute("data-can-edit", "true");
    expect(screen.getByTestId("settings-row")).toHaveTextContent("org_mine");
    expect(screen.getByTestId("delete-form")).toHaveAttribute(
      "data-organization",
      "My Own Workspace",
    );
    expect(document.body.textContent).toMatch(/your account stays/i);
    expect(screen.queryByText("k@example.com")).toBeNull();
  });

  it("offers an admin the name and the URL, and no way to destroy anything", async () => {
    standIn([{ ...MINE, role: "admin" }], "org_mine");
    render(await OrganizationSettingsPage());
    expect(headings()).toEqual(["name", "url", "organization id"]);
    expect(screen.queryByTestId("delete-form")).toBeNull();
    expect(screen.getByTestId("name-form")).toHaveAttribute("data-can-edit", "true");
    expect(screen.getByTestId("url-form")).toHaveAttribute("data-can-edit", "true");
  });

  // Each action refuses an organization other than the one it is handed, so
  // what it is handed has to be the one this page names — the check is only
  // as good as the id on this side of it.
  it("hands every form the id of the organization it names", async () => {
    render(await OrganizationSettingsPage());
    for (const form of ["name-form", "url-form", "delete-form"]) {
      expect(screen.getByTestId(form)).toHaveAttribute("data-organization-id", "org_mine");
    }
  });

  // ⚠ **THE BUG THIS PAGE SHIPPED WITH.** It read `/me` — always the signed-in
  // person's OWN organization — while the rename sent the ACTIVE one upstream.
  // Switched into somebody else's organization, the box showed your
  // organization's name and Save renamed theirs.
  describe("standing in somebody else's organization", () => {
    const THEIRS: OrganizationEntry = {
      organizationId: "org_theirs",
      slug: "theirs",
      name: "Analytical Engines",
      ownerEmail: "owner@example.com",
      ownerDisplayName: "Grace Hopper",
      // An ADMIN, so the view still renders and the assertions below stay
      // about which organization the page names. A member is refused
      // outright; that is the test after these.
      role: "admin",
    };

    beforeEach(() => {
      standIn([MINE, THEIRS], "org_theirs");
    });

    it("shows its name and its URL, and never yours", async () => {
      render(await OrganizationSettingsPage());
      expect(screen.getByTestId("name-form")).toHaveAttribute("data-initial", "Analytical Engines");
      expect(screen.getByTestId("url-form")).toHaveAttribute("data-slug", "theirs");
      expect(screen.getByTestId("settings-row")).toHaveTextContent("org_theirs");
      expect(document.body.innerHTML).not.toContain("My Own Workspace");
      expect(document.body.innerHTML).not.toContain("mine");
      // The organization's name, never its owner's.
      expect(document.body.innerHTML).not.toContain("Grace Hopper");
      expect(document.body.innerHTML).not.toContain("owner@example.com");
    });

    it("offers no danger zone in an organization you do not own", async () => {
      render(await OrganizationSettingsPage());
      expect(screen.queryByTestId("delete-form")).toBeNull();
      expect(screen.queryByText(/danger zone/i)).toBeNull();
    });

    // ⚠ A member holds no organization-wide powers, so organization settings
    // are not theirs to read either. Auth refuses the write below admin; this
    // is the console refusing the page before anybody presses anything.
    it("refuses the page outright to a member", async () => {
      standIn([MINE, { ...THEIRS, role: "member" }], "org_theirs");
      render(await OrganizationSettingsPage());
      expect(screen.getByTestId("access-denied")).toBeInTheDocument();
      expect(document.body.textContent).toMatch(
        /contact grace hopper, the organization.s owner, to request access/i,
      );
      expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
        "href",
        "/theirs",
      );
      expect(screen.queryByTestId("name-form")).toBeNull();
      expect(screen.queryByTestId("url-form")).toBeNull();
    });

    it("shows yours again once you switch back", async () => {
      standIn([MINE, THEIRS], "org_mine");
      render(await OrganizationSettingsPage());
      expect(screen.getByTestId("name-form")).toHaveAttribute("data-initial", "My Own Workspace");
      expect(screen.getByTestId("url-form")).toHaveAttribute("data-slug", "mine");
    });
  });

  // Any organization you own is yours to name, to re-address and to delete: a
  // second one, or one handed to you by its previous owner.
  it("offers everything in any organization the caller owns", async () => {
    standIn(
      [
        MINE,
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
    expect(screen.getByTestId("name-form")).toHaveAttribute("data-initial", "Second Shop");
    expect(screen.getByTestId("name-form")).toHaveAttribute("data-organization-id", "org_handed_over");
    expect(screen.getByTestId("url-form")).toHaveAttribute("data-slug", "second-shop");
    expect(screen.getByTestId("url-form")).toHaveAttribute("data-organization-id", "org_handed_over");
    const deletion = screen.getByTestId("delete-form");
    expect(deletion).toHaveAttribute("data-organization", "Second Shop");
    expect(deletion).toHaveAttribute("data-organization-id", "org_handed_over");
    expect(document.body.innerHTML).not.toContain("My Own Workspace");
  });
});
