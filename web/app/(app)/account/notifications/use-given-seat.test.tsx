// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ActionResult } from "@/lib/action-result";


import type { GivenSeat } from "@/lib/types/given-seat";

const markSeatSeenAction = vi.hoisted(() =>
  vi.fn<(projectId: string) => Promise<ActionResult>>(async () => ({ ok: true })),
);
vi.mock("./seat-actions", () => ({ markSeatSeenAction }));
const removeGivenSeat = vi.hoisted(() => vi.fn());
vi.mock("@/lib/store", () => ({ useRemoveGivenSeat: () => removeGivenSeat }));
const push = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", async (importOriginal) => ({
  ...(await importOriginal<typeof import("next/navigation")>()),
  useRouter: () => ({ push }),
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn() } }));

import { toast } from "sonner";

import { useGivenSeat } from "./use-given-seat";

const SEAT: GivenSeat = {
  projectId: "project_support",
  projectName: "Support agent",
  projectSlug: "support-agent",
  organizationId: "org_acme",
  organizationName: "Acme",
  organizationSlug: "acme",
  role: "member",
  giverEmail: "ada@example.test",
  giverDisplayName: null,
  createdAt: "2026-10-21T00:00:00Z",
};

/** A click on Open, and the spy its default is prevented through. */
const click = (over: Partial<MouseEvent> = {}) => {
  const preventDefault = vi.fn();
  const event = {
    button: 0,
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    preventDefault,
    ...over,
  } as never;
  return { event, preventDefault };
};

beforeEach(() => {
  vi.clearAllMocks();
  markSeatSeenAction.mockResolvedValue({ ok: true });
});

// Open leaves its row behind — the bell closes, or the page is left — so its
// refusal is said by a toast; Dismiss's is said on the row, which stays.
describe("useGivenSeat", () => {
  it("leads to the project by its id, which outlasts a rename", () => {
    const { result } = renderHook(() => useGivenSeat(SEAT));
    expect(result.current.href).toBe("/acme/project_support");
  });

  it("closes the bell, marks the seat, then leads to the project, saying it is on its way", async () => {
    const leave = vi.fn();
    const { result } = renderHook(() => useGivenSeat(SEAT, { onLeave: leave }));
    const { event, preventDefault } = click();
    await act(async () => result.current.open(event));
    expect(preventDefault).toHaveBeenCalled();
    expect(leave).toHaveBeenCalled();
    expect(removeGivenSeat).toHaveBeenCalledWith("project_support");
    expect(push).toHaveBeenCalledWith("/acme/project_support");
    expect(result.current.opening).toBe(true);
  });

  it("says Open's refusal by a toast, from the page as from the bell, and still leads on", async () => {
    markSeatSeenAction.mockResolvedValue({ ok: false, error: "Too many requests — slow down a moment." });
    const { result } = renderHook(() => useGivenSeat(SEAT));
    await act(async () => result.current.open(click().event));
    expect(toast.error).toHaveBeenCalledWith("Too many requests — slow down a moment.");
    expect(result.current.error).toBeNull();
    expect(removeGivenSeat).not.toHaveBeenCalled();
    expect(push).toHaveBeenCalledWith("/acme/project_support");
  });

  it("leaves a click for a new tab to the browser, marking the seat beside it", async () => {
    markSeatSeenAction.mockResolvedValue({ ok: false, error: "Your session expired — sign in again." });
    const leave = vi.fn();
    const { result } = renderHook(() => useGivenSeat(SEAT, { onLeave: leave }));
    const { event, preventDefault } = click({ ctrlKey: true });
    await act(async () => result.current.open(event));
    expect(preventDefault).not.toHaveBeenCalled();
    expect(leave).not.toHaveBeenCalled();
    expect(markSeatSeenAction).toHaveBeenCalledWith("project_support");
    expect(toast.error).toHaveBeenCalledWith("Your session expired — sign in again.");
    expect(push).not.toHaveBeenCalled();
    expect(result.current.opening).toBe(false);
  });

  it("moves the focus off the row before the row goes, once the mark is answered", async () => {
    const order: string[] = [];
    removeGivenSeat.mockImplementationOnce(() => order.push("removed"));
    const { result } = renderHook(() =>
      useGivenSeat(SEAT, { onGone: () => order.push("focus moved") }),
    );
    await act(async () => result.current.dismiss());
    expect(order).toEqual(["focus moved", "removed"]);
  });

  it("says a refused Dismiss on the row, which stays", async () => {
    markSeatSeenAction.mockResolvedValue({ ok: false, error: "Your session expired — sign in again." });
    const gone = vi.fn();
    const { result } = renderHook(() => useGivenSeat(SEAT, { onGone: gone }));
    await act(async () => result.current.dismiss());
    expect(result.current.error).toBe("Your session expired — sign in again.");
    expect(toast.error).not.toHaveBeenCalled();
    expect(gone).not.toHaveBeenCalled();
    expect(removeGivenSeat).not.toHaveBeenCalled();
  });

  it("says a Dismiss that never reached the server on the row", async () => {
    markSeatSeenAction.mockRejectedValue(new Error("offline"));
    const { result } = renderHook(() => useGivenSeat(SEAT));
    await act(async () => result.current.dismiss());
    expect(result.current.error).toBe("Couldn't update that. Try again.");
    expect(removeGivenSeat).not.toHaveBeenCalled();
  });
});
