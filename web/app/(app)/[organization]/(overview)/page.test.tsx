// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ActionResult } from "@/lib/action-result";
import type { OrganizationEntry } from "@/lib/server/entities/organization";

// The action, not its effects: which organization the page hands it is the
// assertion, and the action refuses any other.
const leaveOrganizationAction = vi.hoisted(() =>
  vi.fn<(organizationId: string) => Promise<ActionResult>>(async () => ({ ok: true })),
);
vi.mock("../members/actions", () => ({ leaveOrganizationAction }));

// The title as the real header draws it, the page's one `h1`.
vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title?: string; action?: React.ReactNode }) => (
    <div data-testid="page-header">
      <h1>{title}</h1>
      {action}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ refresh: () => {} }) }));
// The button gates itself by the store's role; here it stands where the page draws it.
vi.mock("@/components/create-project", () => ({
  CreateProjectAction: () => <button type="button">New project</button>,
}));

let mockContext: {
  organizations: OrganizationEntry[];
  activeOrganizationId: string;
} | null;

// The real `activeOrganization`: which entry the page names is the assertion.
vi.mock("@/lib/server/data", async () => {
  const { activeOrganization } = await vi.importActual<
    typeof import("@/lib/server/entities/organization")
  >("@/lib/server/entities/organization");
  return {
    activeOrganization,
    getServerContext: async () => mockContext,
    fetchProjectListing: async () => mockListing,
  };
});

let mockListing:
  | { kind: "ok"; projects: { id: string; slug: string; name: string; role: string }[] }
  | { kind: "unavailable" };

import OrganizationOverviewPage from "./page";

async function renderPage() {
  render(await OrganizationOverviewPage());
}

const OWNED: OrganizationEntry = {
  organizationId: "org_1",
  slug: "ada-works",
  name: "Ada Works",
  ownerEmail: "ada@example.test",
  ownerDisplayName: "Ada",
  role: "owner",
};

beforeEach(() => {
  vi.clearAllMocks();
  mockContext = { organizations: [OWNED], activeOrganizationId: "org_1" };
  mockListing = { kind: "ok", projects: [{ id: "project_1", slug: "web", name: "Web", role: "owner" }] };
});

describe("OrganizationOverviewPage", () => {
  it("shows the outage card when the context is unavailable", async () => {
    mockContext = null;
    await renderPage();
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });

  it("shows the outage card when auth names an organization it does not list", async () => {
    mockContext = { organizations: [OWNED], activeOrganizationId: "org_gone" };
    await renderPage();
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });

  it("welcomes the person to the organization, its first actions and its pages", async () => {
    await renderPage();

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Welcome");
    expect(screen.getByText(/You.re working in/)).toHaveTextContent("You’re working in Ada Works.");
    // New project is the first of the actions, never the header's.
    expect(within(screen.getByTestId("page-header")).queryAllByRole("button")).toHaveLength(0);
    const around = within(screen.getByRole("region", { name: "Getting around" }));
    expect(around.getAllByRole("button")[0]).toHaveTextContent("New project");
    expect(around.getAllByRole("link").map((link) => [link.textContent, link.getAttribute("href")])).toEqual([
      ["Invite a member", "/ada-works/members"],
      ["Send a first trace", "/docs/api/sdks"],
      ["Read the docs", "/docs"],
    ]);
    expect(screen.queryByRole("region", { name: "Quick access" })).toBeNull();
    // Its projects by name alone.
    const projects = screen.getByRole("region", { name: "Projects" });
    expect(within(projects).getByRole("link", { name: "Web" })).toHaveAttribute("href", "/ada-works/web");
    expect(projects).not.toHaveTextContent("Owner");
    expect(screen.queryByText("ada@example.test")).toBeNull();
    expect(screen.queryByText("org_1")).toBeNull();
  });

  it.each<[string, typeof mockListing, string]>([
    ["none", { kind: "ok", projects: [] }, "No projects yet."],
    ["an unread listing", { kind: "unavailable" }, "The projects could not be loaded."],
  ])("says so where it holds %s, not that it holds none it may", async (_, listing, said) => {
    mockListing = listing;
    await renderPage();
    expect(screen.getByRole("region", { name: "Projects" })).toHaveTextContent(said);
  });

  it("offers a member no invitation", async () => {
    mockContext = { organizations: [{ ...OWNED, role: "member" }], activeOrganizationId: "org_1" };
    await renderPage();
    expect(screen.queryByRole("link", { name: "Invite a member" })).toBeNull();
    expect(screen.getByRole("link", { name: "Read the docs" })).toBeInTheDocument();
  });

  it("names the organization the caller stands in, not their own", async () => {
    mockContext = {
      organizations: [
        { ...OWNED, ownerEmail: "admin@example.test", ownerDisplayName: "Admin" },
        {
          organizationId: "org_2",
          slug: "founder-labs",
          name: "Founder Labs",
          ownerEmail: "alex@example.test",
          ownerDisplayName: "Alex Founder",
          role: "admin",
        },
      ],
      activeOrganizationId: "org_2",
    };
    await renderPage();

    // ⚠ The organization's NAME, never its owner's name or address. `Alex
    // Founder` is a human; this heading names an organization, and the two are
    // only ever the same by coincidence.
    expect(screen.getByText(/You.re working in/)).toHaveTextContent("Founder Labs");
    expect(screen.queryByText("Alex Founder")).toBeNull();
    expect(screen.queryByText("alex@example.test")).toBeNull();
    expect(screen.queryByText("Ada Works")).toBeNull();
  });

  it("shows the organization's own name, whoever owns it", async () => {
    mockContext = {
      organizations: [{ ...OWNED, name: "Difference Engine" }],
      activeOrganizationId: "org_1",
    };
    await renderPage();

    expect(screen.getByText("Difference Engine")).toBeInTheDocument();
    expect(screen.queryByText("ada@example.test")).toBeNull();
  });

  // ⚠ The door out is shut to the OWNER: an organization is handed to an
  // admin before its owner leaves, because it cannot be left with nobody
  // holding it.
  it("offers the owner no way to leave", async () => {
    await renderPage();
    expect(screen.queryByRole("button", { name: /^leave/i })).toBeNull();
  });

  it.each(["admin", "member"] as const)(
    "offers an organization %s the way out, naming what they leave",
    async (role) => {
      mockContext = {
        organizations: [{ ...OWNED, name: "Difference Engine", role }],
        activeOrganizationId: "org_1",
      };
      await renderPage();
      expect(
        screen.getByRole("button", { name: "Leave organization: Difference Engine" }),
      ).toBeInTheDocument();
    },
  );

  // ⚠ The button names the organization this page rendered, so the leave has
  // to be aimed at that one — the action refuses any other, including the
  // one its request resolves once this page has gone stale.
  it("leaves the organization it names, and no other", async () => {
    mockContext = {
      organizations: [
        OWNED,
        { ...OWNED, organizationId: "org_2", name: "Difference Engine", role: "member" },
      ],
      activeOrganizationId: "org_2",
    };
    await renderPage();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Leave organization: Difference Engine" }));
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Leave" }));
    });
    expect(leaveOrganizationAction).toHaveBeenCalledWith("org_2");
  });
});
