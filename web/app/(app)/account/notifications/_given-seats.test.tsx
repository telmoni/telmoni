// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { GivenSeat } from "@/lib/types/given-seat";

const markSeatSeenAction = vi.hoisted(() =>
  vi.fn<(projectId: string) => Promise<{ error: string | null }>>(async () => ({ error: null })),
);
vi.mock("./seat-actions", () => ({ markSeatSeenAction }));

const removeGivenSeat = vi.hoisted(() => vi.fn());
vi.mock("@/lib/store", () => ({
  useRemoveGivenSeat: () => removeGivenSeat,
}));
vi.mock("@/components/local-time", () => ({
  LocalTime: ({ iso }: { iso: string }) => <time>{iso}</time>,
}));
const push = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", async (importOriginal) => ({
  ...(await importOriginal<typeof import("next/navigation")>()),
  useRouter: () => ({ push }),
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn() } }));

import { toast } from "sonner";

import { GivenSeatsSection } from "./_given-seats";

const SEAT: GivenSeat = {
  projectId: "project_support",
  projectName: "Support agent",
  projectSlug: "support-agent",
  organizationId: "org_acme",
  organizationName: "Acme",
  organizationSlug: "acme",
  role: "member",
  giverEmail: "ada@example.test",
  giverDisplayName: "Ada Lovelace",
  createdAt: "2026-10-21T00:00:00Z",
};

beforeEach(() => {
  vi.clearAllMocks();
  markSeatSeenAction.mockResolvedValue({ error: null });
});

const press = async (name: string) => {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name }));
  });
};

// Each row names the project, where it is and at what role, and who gave the
// seat; Open leads to the project and Dismiss clears the row, both marking it
// seen for the bell.
describe("GivenSeatsSection", () => {
  it("draws nothing with no seat waiting", () => {
    const { container } = render(<GivenSeatsSection seats={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("lists the project, its organization and role, who gave the seat, and the way to it", () => {
    render(<GivenSeatsSection seats={[SEAT]} />);
    const row = screen.getByRole("row", { name: /Support agent/ });
    expect(row).toHaveTextContent("Acme · Member");
    expect(row).toHaveTextContent("Ada Lovelace");
    expect(within(row).getByRole("link", { name: "Open Support agent in Acme" })).toHaveAttribute(
      "href",
      "/acme/project_support",
    );
  });

  it("marks the seat seen and takes the row off the store on Dismiss", async () => {
    render(<GivenSeatsSection seats={[SEAT]} />);
    await press("Dismiss Support agent in Acme");
    expect(markSeatSeenAction).toHaveBeenCalledWith("project_support");
    expect(removeGivenSeat).toHaveBeenCalledWith("project_support");
  });

  it("takes a plain click's default for itself, and leaves one for a new tab to the browser", () => {
    markSeatSeenAction.mockReturnValue(new Promise(() => {}));
    render(<GivenSeatsSection seats={[SEAT]} />);
    const open = screen.getByRole("link", { name: "Open Support agent in Acme" });
    expect(fireEvent.click(open, { ctrlKey: true }), "a new tab's click was taken").toBe(true);
    expect(fireEvent.click(open), "the click led away before the mark").toBe(false);
  });

  it("says the project is on its way while the mark is answered", async () => {
    markSeatSeenAction.mockReturnValue(new Promise(() => {}));
    render(<GivenSeatsSection seats={[SEAT]} />);
    await act(async () => {
      fireEvent.click(screen.getByRole("link", { name: "Open Support agent in Acme" }));
    });
    expect(screen.getByRole("link", { name: "Opening Support agent in Acme" })).toHaveTextContent(
      "Opening…",
    );
  });

  it("hands the focus to the next row's Open as a dismissed row goes", async () => {
    const billing = { ...SEAT, projectId: "project_billing", projectName: "Billing agent" };
    render(<GivenSeatsSection seats={[SEAT, billing]} />);
    const dismiss = screen.getByRole("button", { name: "Dismiss Support agent in Acme" });
    dismiss.focus();
    await act(async () => {
      fireEvent.click(dismiss);
    });
    expect(document.activeElement).toBe(
      screen.getByRole("link", { name: "Open Billing agent in Acme" }),
    );
  });

  it("marks the seat seen on Open, and only then leads to the project", async () => {
    let answered = false;
    markSeatSeenAction.mockImplementationOnce(async () => {
      expect(push, "it led away before the mark was answered").not.toHaveBeenCalled();
      answered = true;
      return { error: null };
    });
    render(<GivenSeatsSection seats={[SEAT]} />);
    await act(async () => {
      fireEvent.click(screen.getByRole("link", { name: "Open Support agent in Acme" }));
    });
    expect(answered).toBe(true);
    expect(removeGivenSeat).toHaveBeenCalledWith("project_support");
    expect(push).toHaveBeenCalledWith("/acme/project_support");
  });

  it("still leads to the project when the mark is refused, the row staying", async () => {
    markSeatSeenAction.mockResolvedValueOnce({ error: "Too many requests — slow down a moment." });
    render(<GivenSeatsSection seats={[SEAT]} />);
    await act(async () => {
      fireEvent.click(screen.getByRole("link", { name: "Open Support agent in Acme" }));
    });
    expect(removeGivenSeat).not.toHaveBeenCalled();
    expect(toast.error).toHaveBeenCalledWith("Too many requests — slow down a moment.");
    expect(push).toHaveBeenCalledWith("/acme/project_support");
  });

  it("leaves a click for a new tab to the browser, and marks the seat beside it", async () => {
    render(<GivenSeatsSection seats={[SEAT]} />);
    await act(async () => {
      fireEvent.click(screen.getByRole("link", { name: "Open Support agent in Acme" }), { ctrlKey: true });
    });
    expect(markSeatSeenAction).toHaveBeenCalledWith("project_support");
    expect(push).not.toHaveBeenCalled();
  });

  it("keeps the row and says why when the mark is refused", async () => {
    markSeatSeenAction.mockResolvedValue({ error: "Your session expired — sign in again." });
    render(<GivenSeatsSection seats={[SEAT]} />);
    await press("Dismiss Support agent in Acme");
    expect(screen.getByRole("alert")).toHaveTextContent("Your session expired");
    expect(removeGivenSeat).not.toHaveBeenCalled();
  });
});
