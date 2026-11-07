// @vitest-environment jsdom
import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { IdentityContext } from "@/lib/server/entities/identity-context";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => <div data-testid="page-header">{title}</div>,
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

vi.mock("next/navigation", () => ({
  notFound: () => {
    throw new Error("NEXT_NOT_FOUND");
  },
  useRouter: () => ({ refresh: () => {} }),
}));

vi.mock("./actions", () => ({ setProjectCapsAction: vi.fn() }));

let mockContext: unknown;
let mockIdentity: IdentityContext | null = null;
let mockLimits: unknown = null;

vi.mock("@/lib/server/data", async () => {
  const { activeOrganization } = await vi.importActual<
    typeof import("@/lib/server/entities/organization")
  >("@/lib/server/entities/organization");
  return {
    activeOrganization,
    getServerContext: async () => mockContext,
    identityContext: async () => mockIdentity,
  };
});
vi.mock("@/lib/server/entities/limits", () => ({ fetchLimits: async () => mockLimits }));

import LimitsPage from "./page";

const NONE = {
  spans: null,
  content_bytes: null,
  alerts: null,
  monitors: null,
  objectives: null,
  alert_deliveries_per_day: null,
};

function asRole(role: "owner" | "admin" | "member") {
  mockContext = {
    person: { userId: "user_1", email: "ada@example.test" },
    organizations: [
      { organizationId: "org_1", slug: "acme", name: "Acme", ownerEmail: "o@example.test", role },
    ],
    activeOrganizationId: "org_1",
    memberships: [],
    incomingInvites: [],
    flags: {},
  };
  mockIdentity = { userId: "user_1", organizationId: "org_1", role } as IdentityContext;
}

beforeEach(() => {
  asRole("member");
  mockLimits = {
    plan: { spans: 1_000_000, content_bytes: 1_000_000_000, alerts: 50, alert_deliveries_per_day: 2_000 },
    projects: [
      {
        id: "project_1",
        name: "Support bot",
        caps: { ...NONE, spans: 200_000, alerts: 10 },
        used: { spans: 12_500, content_bytes: 4_000_000, alerts: 3, monitors: 1, objectives: 0, alert_deliveries_today: null },
      },
      {
        id: "project_2",
        name: "Research",
        caps: NONE,
        used: { spans: null, content_bytes: 0, alerts: 0, monitors: 0, objectives: 0, alert_deliveries_today: null },
      },
    ],
  };
});

// Every member reads how the pool is divided; its owners and admins set it.
describe("the Limits page", () => {
  it("shows each project's use against its cap, and the plan's pool", async () => {
    render(await LimitsPage());
    expect(screen.getByText(/The plan holds 1,000,000 spans a month/)).toBeInTheDocument();
    const capped = screen.getByRole("cell", { name: "Support bot" }).closest("tr") as HTMLElement;
    expect(within(capped).getByText("12,500 of 200,000")).toBeInTheDocument();
    expect(within(capped).getByText("3 of 10")).toBeInTheDocument();
    expect(within(capped).getByText("4 MB")).toBeInTheDocument();
    const shared = screen.getByRole("cell", { name: "Research" }).closest("tr") as HTMLElement;
    expect(within(shared).getByText("—")).toBeInTheDocument();
    expect(within(shared).getByText("Shares the pool")).toBeInTheDocument();
  });

  it("offers caps to an owner or admin, and to no member", async () => {
    const { unmount } = render(await LimitsPage());
    expect(screen.queryByRole("button", { name: /Set .*'s caps/ })).toBeNull();
    unmount();
    asRole("admin");
    render(await LimitsPage());
    expect(screen.getByRole("button", { name: "Set Support bot's caps" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Set Research's caps" })).toBeInTheDocument();
  });

  it("says when the limits could not be read", async () => {
    mockLimits = null;
    render(await LimitsPage());
    expect(screen.getByText("The limits could not be read. Reload to try again.")).toBeInTheDocument();
  });
});
