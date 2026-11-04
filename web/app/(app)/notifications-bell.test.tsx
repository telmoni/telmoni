// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ActionResult } from "@/lib/action-result";

import type { GivenSeat } from "@/lib/types/given-seat";

const markSeatSeenAction = vi.hoisted(() =>
  vi.fn<(projectId: string) => Promise<ActionResult>>(async () => ({ ok: true })),
);
vi.mock("@/app/(app)/settings/notifications/seat-actions", () => ({ markSeatSeenAction }));
vi.mock("@/app/(app)/settings/notifications/invite-actions", () => ({
  acceptIncomingInviteAction: vi.fn(),
  declineIncomingInviteAction: vi.fn(),
}));
vi.mock("@/components/audit-exports", () => ({
  useAuditExportsWatcher: () => {},
  downloadExport: vi.fn(),
}));
const push = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", async (importOriginal) => ({
  ...(await importOriginal<typeof import("next/navigation")>()),
  useRouter: () => ({ push }),
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn() } }));

// The places the bell stands in, and their feeds: none unless a test sets one.
const place = vi.hoisted(() => ({
  project: null as null | Record<string, string>,
  organization: null as null | Record<string, string>,
  feeds: new Map<string, { items: unknown[]; unread: number }>(),
}));
vi.mock("@/lib/use-scope-feed", () => ({
  useBellPlaces: () => ({ project: place.project, organization: place.organization }),
  useScopeFeed: (scope: { kind: string } | null) => ({
    feed: scope ? (place.feeds.get(scope.kind) ?? null) : null,
    read: true,
  }),
}));

import { setPageCovered } from "@/lib/page-cover";
import { StoreProvider } from "@/lib/store/provider";

import { NotificationsBell } from "@/components/notifications-bell";

const seat = (projectId: string, projectName: string): GivenSeat => ({
  projectId,
  projectName,
  projectSlug: projectId,
  organizationId: "org_acme",
  organizationName: "Acme",
  organizationSlug: "acme",
  role: "member",
  giverEmail: "ada@example.test",
  giverDisplayName: "Ada Lovelace",
  createdAt: "2026-10-21T00:00:00Z",
});

const SUPPORT = seat("project_support", "Support agent");
const BILLING = seat("project_billing", "Billing agent");

/** The bell over a store holding `seats`, opened from the keyboard. */
function openBell(seats: GivenSeat[]): HTMLElement {
  render(
    <StoreProvider user={null} flags={{}} projects={[]} givenSeats={seats}>
      <NotificationsBell />
    </StoreProvider>,
  );
  fireEvent.keyDown(screen.getByTestId("notifications-bell"), { key: "Enter" });
  return screen.getByRole("menu");
}

beforeEach(() => {
  vi.clearAllMocks();
  markSeatSeenAction.mockResolvedValue({ ok: true });
  place.project = null;
  place.organization = null;
  place.feeds.clear();
});

// One list: the person's own, then a preview of the place's notices — the
// project's and the organization's — newest first, each leading to the page
// it is about; the badge counts the person's own alone, and See all leads to
// the place's own page.
describe("NotificationsBell: the place's notices", () => {
  const notice = (id: string, kind: string, created_at: string, metadata: unknown = null) => ({
    id,
    kind,
    title: `Notice ${id}`,
    body: "What happened.",
    metadata,
    page: null,
    read_at: null,
    created_at,
  });
  const INCIDENT = notice("n1", "incident_opened", "2026-11-04T10:00:00.000000Z", {
    incident_id: "5d4f5c68-0000-4000-8000-000000000001",
    rule_id: "r",
  });
  const JOINED = notice("n2", "member_added", "2026-11-04T08:00:00.000000Z");
  const PLAN = notice("o1", "organization_alert", "2026-11-04T09:00:00.000000Z");
  beforeEach(() => {
    place.project = { kind: "project", projectId: "project_web", organization: "acme", project: "web", name: "Web" };
    place.organization = { kind: "organization", organizationId: "org_acme", organization: "acme", name: "Acme" };
    place.feeds.set("project", { items: [INCIDENT, JOINED], unread: 2 });
    place.feeds.set("organization", { items: [PLAN], unread: 1 });
  });

  it("lists every feed's notices newest first, after the person's own, each where it was raised", () => {
    const menu = openBell([SUPPORT]);
    // The seat alone: three notices unread, none of them counted.
    expect(screen.getByTestId("notifications-bell-count")).toHaveTextContent("1");
    expect(within(menu).queryByRole("menuitemradio")).toBeNull();
    const rows = within(menu).getAllByTestId("notifications-menu-notice");
    expect(rows.map((row) => row.textContent)).toEqual([
      expect.stringMatching(/^WebNotice n1/),
      expect.stringMatching(/^OrganizationNotice o1/),
      expect.stringMatching(/^WebNotice n2/),
    ]);
    expect(within(menu).getByRole("menuitem", { name: /Open Support agent/ })).toBeInTheDocument();
  });

  it("leads each notice to the page it is about", () => {
    const menu = openBell([]);
    const [incident, plan, joined] = within(menu).getAllByTestId("notifications-menu-notice");
    expect(incident).toHaveAttribute("href", expect.stringContaining("/acme/web/alerts?incident="));
    expect(plan).toHaveAttribute("href", "/acme/notifications");
    expect(joined).toHaveAttribute("href", "/acme/web/members");
    expect(within(menu).getByTestId("notifications-menu-all")).toHaveAttribute("href", "/acme/web/notifications");
    expect(screen.queryByTestId("notifications-bell-count"), "no unread notice is counted").toBeNull();
  });

  it("reads the organization's feed alone off a project, and none off an organization's pages", () => {
    place.project = null;
    const { unmount } = render(
      <StoreProvider user={null} flags={{}} projects={[]} givenSeats={[]}>
        <NotificationsBell />
      </StoreProvider>,
    );
    fireEvent.keyDown(screen.getByTestId("notifications-bell"), { key: "Enter" });
    const [plan] = within(screen.getByRole("menu")).getAllByTestId("notifications-menu-notice");
    expect(plan).toHaveAttribute("href", "/acme/notifications");
    expect(within(screen.getByRole("menu")).getAllByTestId("notifications-menu-notice")).toHaveLength(1);
    expect(screen.getByTestId("notifications-menu-all")).toHaveAttribute("href", "/acme/notifications");
    unmount();
    place.organization = null;
    const menu = openBell([]);
    expect(within(menu).queryByTestId("notifications-menu-notice")).toBeNull();
    expect(within(menu).getByTestId("notifications-menu-empty")).toBeInTheDocument();
    expect(within(menu).getByTestId("notifications-menu-all")).toHaveAttribute("href", "/settings/notifications");
  });
});

// A seat given waits in the bell with Open and Dismiss, both menu items the
// arrow keys reach, each named for its project and organization, which project
// names alone are not, and described by the row's own words.
describe("NotificationsBell: a seat given", () => {
  it("names each seat's controls by project and organization, and describes them by its row", () => {
    const menu = openBell([SUPPORT]);
    const open = within(menu).getByRole("menuitem", { name: "Open Support agent in Acme" });
    expect(open).toHaveAttribute("href", "/acme/project_support");
    expect(open).toHaveAccessibleDescription(/You were added · Acme · Member/);
    expect(
      within(menu).getByRole("menuitem", { name: "Dismiss Support agent in Acme" }),
    ).toHaveAccessibleDescription(/by Ada Lovelace/);
  });

  it("keeps the focus in the menu when a row dismissed from the keyboard goes", async () => {
    const menu = openBell([SUPPORT, BILLING]);
    const dismiss = within(menu).getByRole("menuitem", { name: "Dismiss Support agent in Acme" });
    dismiss.focus();
    await act(async () => {
      fireEvent.keyDown(dismiss, { key: "Enter" });
    });
    expect(markSeatSeenAction).toHaveBeenCalledWith("project_support");
    const left = screen.getByRole("menu");
    expect(within(left).queryByRole("menuitem", { name: /Support agent/ })).toBeNull();
    expect(document.activeElement).toBe(
      within(left).getByRole("menuitem", { name: "Open Billing agent in Acme" }),
    );
  });

  it("hands the focus on to the foot when the last row goes", async () => {
    const menu = openBell([SUPPORT]);
    const dismiss = within(menu).getByRole("menuitem", { name: "Dismiss Support agent in Acme" });
    dismiss.focus();
    await act(async () => {
      fireEvent.keyDown(dismiss, { key: "Enter" });
    });
    expect(document.activeElement).toBe(screen.getByTestId("notifications-menu-all"));
  });

  it("closes the menu, marks the seat, and only then leads to the project on Open", async () => {
    const order: string[] = [];
    markSeatSeenAction.mockImplementationOnce(async () => {
      order.push("marked");
      return { ok: true };
    });
    push.mockImplementationOnce(() => order.push("led away"));
    const menu = openBell([SUPPORT]);
    await act(async () => {
      fireEvent.click(within(menu).getByRole("menuitem", { name: "Open Support agent in Acme" }));
    });
    expect(screen.queryByRole("menu"), "the menu stayed open over the way out").toBeNull();
    expect(order).toEqual(["marked", "led away"]);
    expect(push).toHaveBeenCalledWith("/acme/project_support");
  });
});

describe("NotificationsBell: its key", () => {
  // ⚠ A hint names no key a panel over the page holds off.
  it("names its g sequence, unless a panel covers the page", () => {
    render(
      <StoreProvider user={null} flags={{}} projects={[]} givenSeats={[]}>
        <NotificationsBell />
      </StoreProvider>,
    );
    const bell = screen.getByTestId("notifications-bell");
    expect(bell).toHaveAttribute("title", expect.stringMatching(/^Notifications \(.+\)$/));
    act(() => setPageCovered(true));
    expect(bell).toHaveAttribute("title", "Notifications");
    act(() => setPageCovered(false));
  });
});
