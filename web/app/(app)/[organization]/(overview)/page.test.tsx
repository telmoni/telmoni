// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { incidentHref } from "@/lib/alerts";
import type { OrganizationEntry } from "@/lib/server/entities/organization";
import type {
  HealthIncident,
  OrganizationHealth,
} from "@/lib/server/entities/organization-health";

// The action, not its effects: which organization the page hands it is the
// assertion, and the action refuses any other.
const leaveOrganizationAction = vi.hoisted(() =>
  vi.fn<(organizationId: string) => Promise<{ error: string | null }>>(async () => ({
    error: null,
  })),
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

vi.mock("../notice-actions", () => ({
  markOrganizationReadAction: vi.fn(async () => ({ error: null })),
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ refresh: () => {} }) }));

// A stand-in for the create flow, so a test can say the overview offers none.
vi.mock("@/components/create-project", () => ({
  CreateProjectAction: () => (
    <div data-testid="create-project-action" />
  ),
}));

let mockContext: {
  organizations: OrganizationEntry[];
  activeOrganizationId: string;
} | null;

const fetchNotifications = vi.hoisted(() =>
  vi.fn<() => Promise<{ items: FeedItem[]; unread: number } | null>>(async () => null),
);
const fetchOrganizationHealth = vi.hoisted(() =>
  vi.fn<() => Promise<OrganizationHealth>>(async () => ({ kind: "ok", incidents: [] })),
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
    fetchOrganizationHealth,
    getServerContext: async () => mockContext,
  };
});

/** An open incident on Web, as the server sends it, with the `fields` a test
 *  sets. */
function incident(fields: Partial<HealthIncident>): HealthIncident {
  return {
    id: "incident_1",
    rule_id: "rule_1",
    rule_name: "Slow calls",
    status: "open",
    severity: "alert",
    value: 6200,
    opened_at: "2026-10-27T03:00:00Z",
    acknowledged_at: null,
    acknowledged_by: null,
    resolved_at: null,
    resolved_by: null,
    reopened: 0,
    told_to: [],
    runs: [],
    project: { id: "project_1", slug: "web", name: "Web" },
    ...fields,
  };
}

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
  fetchOrganizationHealth.mockResolvedValue({ kind: "ok", incidents: [] });
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

    // The page is titled by the organization, with no card repeating it.
    expect(screen.getByTestId("page-header")).toHaveTextContent("Ada Works");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Ada Works");
    expect(screen.queryAllByRole("heading", { level: 2 })).toHaveLength(0);
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
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Founder Labs");
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

  // A project is made on the Projects page or from the switcher; the overview
  // is for seeing the organization, so it offers nobody New project.
  it.each(["owner", "admin"] as const)(
    "offers an organization %s no New project here, and no card for none",
    async (role) => {
      mockContext = {
        organizations: [{ ...OWNED, role }],
        activeOrganizationId: "org_1",
      };
      await renderPage();

      expect(screen.queryByTestId("create-project-action")).toBeNull();
      expect(screen.queryByText(/No projects yet/)).toBeNull();
    },
  );

  // ⚠ In the server's order, newest first, each linking to it under its alert.
  it("lists the open incidents, each linking to it under its alert", async () => {
    fetchOrganizationHealth.mockResolvedValue({
      kind: "ok",
      incidents: [
        incident({
          id: "incident_2",
          rule_name: "Spend",
          status: "acknowledged",
          severity: "warning",
          acknowledged_at: "2026-10-27T04:00:00Z",
          acknowledged_by: "user_1",
          project: { id: "project_2", slug: "payments", name: "Payments" },
        }),
        incident({}),
      ],
    });
    await renderPage();

    const section = screen.getByRole("region", { name: "Open incidents" });
    expect(
      within(section)
        .getAllByRole("link")
        .map((link) => [link.textContent, link.getAttribute("href")]),
    ).toEqual([
      ["Spend in Payments", incidentHref("ada-works", "payments", "incident_2")],
      ["Slow calls in Web", incidentHref("ada-works", "web", "incident_1")],
    ]);
    const [spend, slow] = within(section)
      .getAllByRole("link")
      .map((link) => link.closest("div.flex"));
    expect(spend).toHaveTextContent(/Payments · opened .* · acknowledged/);
    expect(spend).toHaveTextContent("Warning");
    expect(slow).toHaveTextContent(/Web · opened /);
    expect(slow).not.toHaveTextContent("acknowledged");
    expect(slow).toHaveTextContent("Alert");
  });

  it.each<[string, OrganizationHealth]>([
    ["all is well", { kind: "ok", incidents: [] }],
    ["the server refuses the person", { kind: "forbidden" }],
  ])("says nothing about incidents when %s", async (_, health) => {
    fetchOrganizationHealth.mockResolvedValue(health);
    await renderPage();
    expect(screen.queryByRole("region", { name: "Open incidents" })).toBeNull();
    expect(screen.queryByText(/incident/i)).toBeNull();
  });

  // Saying nothing would read as all being well.
  it("says the open incidents could not be read, rather than nothing", async () => {
    fetchOrganizationHealth.mockResolvedValue({ kind: "unavailable" });
    await renderPage();
    expect(screen.getByRole("region", { name: "Open incidents" })).toHaveTextContent(
      "The open incidents could not be read.",
    );
  });

  it.each(["owner", "admin", "member"] as const)(
    "asks an organization %s's open incidents of the server, which narrows them",
    async (role) => {
      mockContext = {
        organizations: [{ ...OWNED, role }],
        activeOrganizationId: "org_1",
      };
      await renderPage();
      expect(fetchOrganizationHealth).toHaveBeenCalled();
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
