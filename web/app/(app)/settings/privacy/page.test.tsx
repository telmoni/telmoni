// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { OrganizationEntry } from "@/lib/server/entities/organization";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => (
    <div data-testid="page-header">{title}</div>
  ),
}));

vi.mock("./_sessions", () => ({
  ActiveSessions: () => <div data-testid="sessions" />,
}));

vi.mock("./_analytics", () => ({
  AnalyticsPreference: ({ optIn }: { optIn: boolean }) => (
    <div data-testid="analytics" data-opt-in={String(optIn)} />
  ),
}));
// Whether the deployment has an analytics provider, which decides whether the
// data collection section is drawn at all.
const analyticsConfigured = vi.hoisted(() => vi.fn(() => true));
vi.mock("@/lib/analytics", () => ({ analyticsConfigured }));

vi.mock("./_delete-account", () => ({
  DeleteAccountForm: ({ ownedOrganizations }: { ownedOrganizations: number }) => (
    <div data-testid="delete-account" data-owned={String(ownedOrganizations)} />
  ),
}));

vi.mock("next/navigation", () => ({
  notFound: () => {
    throw new Error("notFound");
  },
}));

let mockContext: {
  person: { analyticsOptIn: boolean };
  organizations: OrganizationEntry[];
} | null = null;
vi.mock("@/lib/server/data", () => ({
  fetchActiveSessions: async () => [],
  getServerContext: async () => mockContext,
}));

vi.mock("@/lib/server/session", () => ({
  getServerSession: async () => ({
    userId: "user_1",
    email: "k@example.com",
    firstName: "K",
    lastName: null,
    sessionRowId: null,
  }),
}));

import AccountPrivacyPage from "./page";

beforeEach(() => {
  mockContext = {
    person: { analyticsOptIn: false },
    organizations: [
      { organizationId: "org_1", slug: "one", name: "One", ownerEmail: "k@example.com", role: "owner" },
    ],
  };
});

describe("AccountPrivacyPage", () => {
  it("lists the live sessions and offers the erasure", async () => {
    render(await AccountPrivacyPage());
    expect(screen.getByTestId("page-header")).toHaveTextContent("Privacy");
    expect(screen.getByTestId("sessions")).toBeInTheDocument();
    expect(screen.getByTestId("delete-account")).toBeInTheDocument();
    expect(screen.getByTestId("analytics")).toBeInTheDocument();
  });

  // A deployment without a provider sends nothing, so the page asks for no
  // consent and opens on the sessions.
  it("draws no data collection section where the deployment has no provider", async () => {
    analyticsConfigured.mockReturnValueOnce(false);
    render(await AccountPrivacyPage());
    expect(screen.queryByTestId("analytics")).toBeNull();
    const headings = screen
      .getAllByRole("heading", { level: 2 })
      .map((h) => h.textContent?.toLowerCase() ?? "");
    expect(headings).toEqual(["active sessions", "danger zone"]);
  });

  // The consent is the person's own, and `/me` always states it; the one way
  // auth does not say is `/me` not answering at all.
  it("defaults to not counted whenever auth does not say", async () => {
    const optIn = () =>
      screen.getByTestId("analytics").getAttribute("data-opt-in");

    mockContext = null;
    const { unmount } = render(await AccountPrivacyPage());
    expect(optIn()).toBe("false");
    unmount();

    mockContext = { person: { analyticsOptIn: false }, organizations: [] };
    const { unmount: close } = render(await AccountPrivacyPage());
    expect(optIn()).toBe("false");
    close();

    mockContext = { person: { analyticsOptIn: true }, organizations: [] };
    render(await AccountPrivacyPage());
    expect(optIn()).toBe("true");
  });

  it("ends with the danger zone, and holds nothing of the profile", async () => {
    render(await AccountPrivacyPage());
    const headings = screen
      .getAllByRole("heading", { level: 2 })
      .map((h) => h.textContent?.toLowerCase() ?? "");
    expect(headings).toEqual(["data collection", "active sessions", "danger zone"]);
    expect(screen.queryByTestId("deleted-organizations")).toBeNull();
    expect(screen.queryByText("k@example.com")).toBeNull();
    expect(screen.queryByText("user_1")).toBeNull();
  });

  // ⚠ Deleting the account takes every organization the person owns on their
  // own, and is refused over one anybody else is in — so the form counts the
  // ones they OWN, and none they merely belong to. A count rather than their
  // names, since twenty names would bury the form. Deleting one organization
  // is its own settings page's job.
  it("counts every organization the account owns, and none it only belongs to", async () => {
    mockContext = {
      person: { analyticsOptIn: false },
      organizations: [
        { organizationId: "org_1", slug: "acme", name: "Acme", ownerEmail: "k@example.com", role: "owner" },
        { organizationId: "org_2", slug: "two", name: "Two", ownerEmail: "k@example.com", role: "owner" },
        { organizationId: "org_3", slug: "theirs", name: "Theirs", ownerEmail: "t@example.com", role: "admin" },
        { organizationId: "org_4", slug: "four", name: "Four", ownerEmail: "m@example.com", role: "member" },
      ],
    };
    render(await AccountPrivacyPage());
    expect(screen.getByTestId("delete-account")).toHaveAttribute("data-owned", "2");
  });

  // Two organizations may carry one name — one made at sign-up, one handed
  // over — and each is still one organization the deletion takes.
  it("counts owned organizations that share a name once each", async () => {
    mockContext = {
      person: { analyticsOptIn: false },
      organizations: [
        { organizationId: "org_1", slug: "one", name: "Acme", ownerEmail: "k@example.com", role: "owner" },
        { organizationId: "org_2", slug: "globex", name: "Globex", ownerEmail: "k@example.com", role: "owner" },
        { organizationId: "org_3", slug: "three", name: "Acme", ownerEmail: "k@example.com", role: "owner" },
      ],
    };
    render(await AccountPrivacyPage());
    expect(screen.getByTestId("delete-account")).toHaveAttribute("data-owned", "3");
  });

  it("counts nothing when it cannot tell what the account owns", async () => {
    mockContext = null;
    render(await AccountPrivacyPage());
    expect(screen.getByTestId("delete-account")).toHaveAttribute("data-owned", "0");
  });

  it("points at the organization's own settings to delete one and keep the account", async () => {
    render(await AccountPrivacyPage());
    expect(document.body.textContent).toMatch(/every organization you own on your own/i);
    expect(document.body.textContent).toMatch(/use that organization's settings/i);
  });
});
