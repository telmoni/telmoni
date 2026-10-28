// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ActionResult } from "@/lib/action-result";

import type { GivenSeat } from "@/lib/types/given-seat";

const markSeatSeenAction = vi.hoisted(() =>
  vi.fn<(projectId: string) => Promise<ActionResult>>(async () => ({ ok: true })),
);
vi.mock("@/app/(app)/account/notifications/seat-actions", () => ({ markSeatSeenAction }));
vi.mock("@/app/(app)/account/notifications/invite-actions", () => ({
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
