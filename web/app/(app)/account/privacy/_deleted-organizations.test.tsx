// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DeletedOrganization } from "@/lib/server/data";

const restoreOrganizationAction = vi.hoisted(() =>
  vi.fn<(organizationId: string) => Promise<{ error?: string; ok?: boolean }>>(async () => ({
    ok: true,
  })),
);
vi.mock("./actions", () => ({ restoreOrganizationAction }));

import { DeletedOrganizations } from "./_deleted-organizations";

const replace = vi.fn();

const MINE: DeletedOrganization = {
  organizationId: "org_mine",
  name: "Acme",
  deletionRequestedAt: "2026-09-23T10:00:00Z",
  eraseAfter: "2026-10-07T10:00:00Z",
  restorable: true,
};
const UNNAMED: DeletedOrganization = {
  organizationId: "org_unnamed",
  name: null,
  deletionRequestedAt: "2026-09-20T10:00:00Z",
  eraseAfter: "2026-10-04T10:00:00Z",
  restorable: true,
};
const CLOSED_BY_TELMONI: DeletedOrganization = {
  organizationId: "org_closed",
  name: "Closed Co",
  deletionRequestedAt: "2026-09-22T10:00:00Z",
  eraseAfter: "2026-10-06T10:00:00Z",
  restorable: false,
};

beforeEach(() => {
  vi.clearAllMocks();
  restoreOrganizationAction.mockResolvedValue({ ok: true });
  vi.stubGlobal("location", { replace });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const mount = (organizations: DeletedOrganization[]) =>
  render(<DeletedOrganizations organizations={organizations} ownerEmail="ada@example.test" />);

describe("DeletedOrganizations", () => {
  it("renders nothing when nothing is being deleted", () => {
    mount([]);
    expect(screen.queryByTestId("deleted-organizations")).toBeNull();
  });

  // The label is what the console calls the organization everywhere else:
  // its name, else its owner's address — and every one of these is the
  // reader's own.
  it("names each organization as the console does, and says until when it can come back", () => {
    mount([MINE, UNNAMED]);
    expect(screen.getByText("Acme")).toBeInTheDocument();
    expect(screen.getByText("ada@example.test")).toBeInTheDocument();
    expect(document.body.textContent).toMatch(/you can restore it until/i);
    const times = [...document.querySelectorAll("time")].map((t) => t.getAttribute("dateTime"));
    expect(times).toContain("2026-10-07T10:00:00Z");
    expect(times).toContain("2026-10-04T10:00:00Z");
    expect(screen.getAllByRole("button", { name: /^restore$/i })).toHaveLength(2);
  });

  // ⚠ An organization Telmoni closed is not the reader's to undo: no button,
  // and support named, so the page never shows a termination as a mistake
  // one click fixes.
  it("offers no restore for an organization Telmoni closed, and names support", () => {
    mount([CLOSED_BY_TELMONI]);
    expect(screen.getByText("Closed Co")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /restore/i })).toBeNull();
    expect(document.body.textContent).toMatch(/contact support to have it restored/i);
  });

  // A full navigation, not a router push: the rail, the selector and the
  // store were all read without the organization that is back.
  it("restores the one pressed and reloads the console", async () => {
    mount([MINE, UNNAMED]);
    await act(async () => {
      fireEvent.click(screen.getAllByRole("button", { name: /^restore$/i })[1]!);
    });
    expect(restoreOrganizationAction).toHaveBeenCalledTimes(1);
    expect(restoreOrganizationAction).toHaveBeenCalledWith("org_unnamed");
    expect(replace).toHaveBeenCalledWith("/console");
  });

  it("shows the refusal and keeps the list", async () => {
    restoreOrganizationAction.mockResolvedValue({
      error: "this organization's restore window has closed; it is being erased",
    });
    mount([MINE]);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /^restore$/i }));
    });
    expect(screen.getByRole("alert")).toHaveTextContent(/restore window has closed/i);
    expect(screen.getByRole("button", { name: /^restore$/i })).toBeEnabled();
    expect(replace).not.toHaveBeenCalled();
  });
});
