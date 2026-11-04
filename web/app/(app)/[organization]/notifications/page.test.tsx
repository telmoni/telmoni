// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { OrganizationRole } from "@/lib/types/enums";

vi.mock("next/navigation", () => ({ notFound: () => { throw new Error("not found"); } }));
vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title: string; action?: React.ReactNode }) => (
    <header>
      <h1>{title}</h1>
      {action}
    </header>
  ),
}));
vi.mock("@/components/access-denied", () => ({
  AccessDenied: ({ what }: { what: string }) => <p>No access to {what}</p>,
}));
vi.mock("@/components/local-time", () => ({ LocalTime: ({ iso }: { iso: string }) => <span>{iso}</span> }));
vi.mock("@/components/mark-read", () => ({ MarkRead: () => <button type="button">Mark read</button> }));
vi.mock("../notice-actions", () => ({ markOrganizationReadAction: vi.fn() }));

let role: string;
const ORGANIZATION = { organizationId: "org_acme", slug: "acme", name: "Acme" };
vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => ({ organizationNotFound: false }),
  activeOrganization: () => ORGANIZATION,
  identityContext: async () => ({ userId: "u", organizationId: "org_acme", role, accessToken: "t" }),
  fetchNotifications: async () => ({
    items: [
      {
        id: "o1",
        kind: "organization_alert",
        title: "Payment failed",
        body: "Update the card.",
        metadata: null,
        page: "/acme/billing",
        read_at: null,
        created_at: "2026-11-04T10:00:00Z",
      },
    ],
    unread: 1,
  }),
}));

import OrganizationNotificationsPage from "./page";

beforeEach(() => {
  role = OrganizationRole.Owner;
});

describe("OrganizationNotificationsPage", () => {
  it("lists the organization's own notices for its owners and admins, each leading to the page it names", async () => {
    render(await OrganizationNotificationsPage());
    expect(screen.getByRole("link", { name: "Payment failed" })).toHaveAttribute("href", "/acme/billing");
    expect(screen.getByRole("button", { name: "Mark read" })).toBeInTheDocument();
  });

  it("tells a member whose they are, and reads none", async () => {
    role = OrganizationRole.Member;
    render(await OrganizationNotificationsPage());
    expect(screen.getByText(/No access to this organization's notifications/)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Payment failed" })).toBeNull();
  });
});
