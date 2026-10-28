// @vitest-environment jsdom
import { act, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ActionResult } from "@/lib/action-result";

import type { GivenSeat } from "@/lib/types/given-seat";

const markSeatSeenAction = vi.hoisted(() =>
  vi.fn<(projectId: string) => Promise<ActionResult>>(async () => ({ ok: true })),
);
vi.mock("@/app/(app)/account/notifications/seat-actions", () => ({ markSeatSeenAction }));

import { useGivenSeats } from "@/lib/store";
import { StoreProvider } from "@/lib/store/provider";

import { GivenSeatSeen } from "./_given-seat-seen";

const seat = (projectId: string): GivenSeat => ({
  projectId,
  projectName: projectId,
  projectSlug: projectId,
  organizationId: "org_acme",
  organizationName: "Acme",
  organizationSlug: "acme",
  role: "member",
  giverEmail: "ada@example.test",
  giverDisplayName: null,
  createdAt: "2026-10-21T00:00:00Z",
});

/** The seats the bell still offers. */
function Waiting() {
  const seats = useGivenSeats() ?? [];
  return <p data-testid="waiting">{seats.map((s) => s.projectId).join(",")}</p>;
}

async function openProject(projectId: string, seats: GivenSeat[]) {
  await act(async () => {
    render(
      <StoreProvider user={null} flags={{}} projects={[]} givenSeats={seats}>
        <GivenSeatSeen projectId={projectId} />
        <Waiting />
      </StoreProvider>,
    );
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  markSeatSeenAction.mockResolvedValue({ ok: true });
});

// Reached by the mail's link or the sidebar rather than the bell's Open, the
// project marks its own seat seen, and the bell stops offering it.
describe("GivenSeatSeen", () => {
  it("marks the seat given on the project opened, and that one alone", async () => {
    await openProject("project_support", [seat("project_support"), seat("project_billing")]);
    expect(markSeatSeenAction).toHaveBeenCalledExactlyOnceWith("project_support");
    expect(screen.getByTestId("waiting")).toHaveTextContent(/^project_billing$/);
  });

  it("asks nothing of a project with no seat waiting on it", async () => {
    await openProject("project_support", [seat("project_billing")]);
    expect(markSeatSeenAction).not.toHaveBeenCalled();
  });

  it("leaves the seat in the bell when the mark is refused", async () => {
    markSeatSeenAction.mockResolvedValue({ ok: false, error: "Too many requests — slow down a moment." });
    await openProject("project_support", [seat("project_support")]);
    expect(screen.getByTestId("waiting")).toHaveTextContent(/^project_support$/);
  });
});
