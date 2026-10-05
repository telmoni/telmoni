// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { OrganizationEntry } from "@/lib/server/entities/organization";

// The action, not its effects: which organization the page hands it is the
// assertion, and the action refuses any other.
const leaveOrganizationAction = vi.hoisted(() =>
  vi.fn<(organizationId: string) => Promise<{ error: string | null }>>(async () => ({
    error: null,
  })),
);
vi.mock("./members/actions", () => ({ leaveOrganizationAction }));

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title?: string; action?: React.ReactNode }) => (
    <div data-testid="page-header">
      {title}
      {action}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

vi.mock("./notice-actions", () => ({
  markOrganizationReadAction: vi.fn(async () => ({ error: null })),
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ refresh: () => {} }) }));

// Which organization the action is aimed at is the assertion; the dialog is
// its own.
vi.mock("@/components/create-project", () => ({
  CreateProjectAction: ({ organizationId }: { organizationId?: string }) => (
    <div data-testid="create-project-action" data-organization-id={organizationId} />
  ),
}));

let mockContext: {
  organizations: OrganizationEntry[];
  activeOrganizationId: string;
} | null;

const fetchNotifications = vi.hoisted(() =>
  vi.fn<() => Promise<{ items: FeedItem[]; unread: number } | null>>(async () => null),
);

type Listing =
  | {
      kind: "ok";
      projects: { id: string; slug: string; name: string; role: string | null }[];
    }
  | { kind: "unavailable" };

const fetchProjectListing = vi.hoisted(() =>
  vi.fn<() => Promise<Listing>>(async () => ({ kind: "ok", projects: [] })),
);

type FeedItem = {
  id: string;
  kind: string;
  title: string;
  body: string;
  read_at: string | null;
  created_at: string;
};

// The real `activeOrganization`: which entry the page names is the assertion.
vi.mock("@/lib/server/data", async () => {
  const { activeOrganization } = await vi.importActual<
    typeof import("@/lib/server/entities/organization")
  >("@/lib/server/entities/organization");
  return {
    activeOrganization,
    fetchNotifications,
    fetchProjectListing,
    getServerContext: async () => mockContext,
  };
});

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
  fetchNotifications.mockResolvedValue(null);
  fetchProjectListing.mockResolvedValue({ kind: "ok", projects: [] });
  mockContext = { organizations: [OWNED], activeOrganizationId: "org_1" };
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

  it("names the organization, and says nothing else about it", async () => {
    await renderPage();

    expect(screen.getByTestId("page-header")).toHaveTextContent("Overview");
    expect(screen.getByText("Ada Works")).toBeInTheDocument();
    expect(screen.queryByText("ada@example.test")).toBeNull();
    expect(screen.queryByText("Ada")).toBeNull();
    expect(screen.queryByText("org_1")).toBeNull();
    expect(screen.queryByText("This organization")).toBeNull();
    expect(screen.queryAllByRole("link")).toHaveLength(0);
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
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("Founder Labs");
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
        screen.getByRole("button", { name: "Leave Difference Engine" }),
      ).toBeInTheDocument();
    },
  );

  // The organization's own notices answer its owner and admins.
  it("shows the owner and admin the organization's notices, and asks for nobody else", async () => {
    fetchNotifications.mockResolvedValue({
      items: [
        {
          id: "n1",
          kind: "organization_alert",
          title: "Action needed",
          body: "Something outside the console needs the owner.",
          read_at: null,
          created_at: "2026-09-28T12:00:00Z",
        },
      ],
      unread: 1,
    });
    await renderPage();
    expect(screen.getByText("Action needed")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Mark read (1)" })).toBeInTheDocument();

    vi.clearAllMocks();
    mockContext = {
      organizations: [{ ...OWNED, role: "admin" }],
      activeOrganizationId: "org_1",
    };
    await renderPage();
    expect(fetchNotifications).toHaveBeenCalled();

    vi.clearAllMocks();
    mockContext = {
      organizations: [{ ...OWNED, role: "member" }],
      activeOrganizationId: "org_1",
    };
    await renderPage();
    expect(fetchNotifications).not.toHaveBeenCalled();
  });

  // ⚠ The console opens here, so the projects are here: each opens its own
  // page, by the slugs the path is spelled with.
  it("lists the organization's projects, each opening its own page", async () => {
    fetchProjectListing.mockResolvedValue({
      kind: "ok",
      projects: [
        { id: "proj_1", slug: "web", name: "Web", role: "admin" },
        { id: "proj_2", slug: "billing-2", name: "Billing", role: null },
      ],
    });
    await renderPage();

    expect(screen.getByRole("link", { name: "Web" })).toHaveAttribute("href", "/ada-works/web");
    expect(screen.getByRole("link", { name: "Billing" })).toHaveAttribute(
      "href",
      "/ada-works/billing-2",
    );
    expect(screen.getByText("Admin")).toBeInTheDocument();
    expect(screen.queryByText("No projects yet.")).toBeNull();
  });

  // A project is made in the organization this page names, by whoever may
  // make one there.
  it.each(["owner", "admin"] as const)(
    "offers an organization %s New project, in the organization it names",
    async (role) => {
      mockContext = {
        organizations: [{ ...OWNED, role }],
        activeOrganizationId: "org_1",
      };
      await renderPage();

      expect(screen.getByTestId("create-project-action")).toHaveAttribute(
        "data-organization-id",
        "org_1",
      );
      expect(
        screen.getByText("Create the first one with New project, above."),
      ).toBeInTheDocument();
    },
  );

  it("offers a member no New project, and says who adds them to one", async () => {
    mockContext = {
      organizations: [{ ...OWNED, role: "member" }],
      activeOrganizationId: "org_1",
    };
    await renderPage();

    expect(screen.queryByTestId("create-project-action")).toBeNull();
    expect(screen.getByText("An owner or admin adds you to a project.")).toBeInTheDocument();
  });

  // An unread listing is not an empty one: saying "no projects yet" would
  // invite somebody to make a second of one they already have.
  it("says the projects could not be loaded, not that there are none", async () => {
    fetchProjectListing.mockResolvedValue({ kind: "unavailable" });
    await renderPage();

    expect(screen.getByText(/could not be loaded/)).toBeInTheDocument();
    expect(screen.queryByText("No projects yet.")).toBeNull();
  });

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
      fireEvent.click(screen.getByRole("button", { name: "Leave Difference Engine" }));
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Leave" }));
    });
    expect(leaveOrganizationAction).toHaveBeenCalledWith("org_2");
  });
});
