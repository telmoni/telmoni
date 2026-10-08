// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Answer = { error?: string; ok?: boolean; purged?: boolean };

const requestOrganizationDeletionCodeAction = vi.hoisted(() =>
  vi.fn<(organizationId: string) => Promise<Answer>>(async () => ({ ok: true })),
);
const deleteOrganizationAction = vi.hoisted(() =>
  vi.fn<(organizationId: string, code: string) => Promise<Answer>>(async () => ({ ok: true })),
);
vi.mock("./deletion-actions", () => ({
  requestOrganizationDeletionCodeAction,
  deleteOrganizationAction,
}));

import { DeleteOrganizationForm } from "./_delete-organization";

const replace = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  requestOrganizationDeletionCodeAction.mockResolvedValue({ ok: true });
  deleteOrganizationAction.mockResolvedValue({ ok: true });
  vi.stubGlobal("location", { replace });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const mount = () =>
  render(
    <DeleteOrganizationForm
      organizationId="org_acme"
      organization="Acme"
      email="dev@example.com"
    />,
  );

const press = async (name: RegExp) => {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name }));
  });
};

const typeCode = async (value: string) => {
  await act(async () => {
    fireEvent.change(screen.getByLabelText(/6-digit code/i), { target: { value } });
  });
};

const deleteButton = () => screen.getByRole("button", { name: /^delete this organization/i });

describe("DeleteOrganizationForm", () => {
  it("holds the delete shut until the six digits are in", async () => {
    mount();
    await press(/send confirmation code/i);
    expect(requestOrganizationDeletionCodeAction).toHaveBeenCalledTimes(1);
    expect(requestOrganizationDeletionCodeAction).toHaveBeenCalledWith("org_acme");

    expect(deleteButton()).toBeDisabled();
    await typeCode("12345");
    expect(deleteButton(), "five digits unlocked it").toBeDisabled();
    await typeCode("123456");
    expect(deleteButton()).toBeEnabled();
    expect(deleteOrganizationAction, "it deleted on a keystroke").not.toHaveBeenCalled();
  });

  // ⚠ **Nobody's account goes with it, the owner's included.** The account
  // form signs out when it is done; this one must not, because the person is
  // still somebody — in whatever organizations they still belong to.
  it("deletes the organization and leaves the owner signed in", async () => {
    mount();
    await press(/send confirmation code/i);
    await typeCode("123456");
    await press(/^delete this organization/i);

    expect(deleteOrganizationAction).toHaveBeenCalledWith("org_acme", "123456");
    expect(screen.getByText("Acme is deleted")).toBeInTheDocument();
    expect(document.body.textContent).toMatch(/your own account is untouched/i);
    // What happens to the data, said once and truthfully: erased within the
    // hour, with no way back.
    expect(document.body.textContent).toMatch(/erased from our systems within the hour/i);
    expect(document.body.textContent).not.toMatch(/restore/i);
    expect(screen.queryByRole("button", { name: /sign out/i })).toBeNull();

    // A full navigation, not a router push: everything the console holds —
    // the rail, the selector, the store — was read for the organization that
    // is gone.
    await press(/continue/i);
    expect(replace).toHaveBeenCalledWith("/console");
    expect(replace).not.toHaveBeenCalledWith("/auth/logout");
  });

  // ⚠ Auth answers 202 whether or not the request's purge hook landed; the
  // sweep retries it every ten minutes. The success screen says so only when
  // it did not, and never on a deployment with no hook, which always lands.
  it("says the cleanup is still being retried only when the purge did not land", async () => {
    deleteOrganizationAction.mockResolvedValue({ ok: true, purged: false });
    const { unmount } = mount();
    await press(/send confirmation code/i);
    await typeCode("123456");
    await press(/^delete this organization/i);
    expect(document.body.textContent).toMatch(/retries every ten minutes/i);
    unmount();

    deleteOrganizationAction.mockResolvedValue({ ok: true, purged: true });
    mount();
    await press(/send confirmation code/i);
    await typeCode("123456");
    await press(/^delete this organization/i);
    expect(document.body.textContent).not.toMatch(/retries every ten minutes/i);
  });

  it("says before the confirm that there is no undo", () => {
    mount();
    expect(document.body.textContent).toMatch(/there is no undo/i);
    expect(document.body.textContent).not.toMatch(/change your mind|restore/i);
  });

  // ⚠ Once it is gone, anything that refreshes the page — the realtime
  // listener, a reconnect — renders whichever organization the console fell
  // back to, and hands this form THAT one's name. The success screen is about
  // the one that was deleted.
  it("keeps naming the deleted organization when the page renders another", async () => {
    const { rerender } = mount();
    await press(/send confirmation code/i);
    await typeCode("123456");
    await press(/^delete this organization/i);

    rerender(
      <DeleteOrganizationForm
        organizationId="org_fallback"
        organization="Fallback Works"
        email="dev@example.com"
      />,
    );
    expect(screen.getByText("Acme is deleted")).toBeInTheDocument();
    expect(screen.queryByText(/fallback works/i)).toBeNull();
  });

  it("shows the refusal and keeps the form", async () => {
    deleteOrganizationAction.mockResolvedValue({
      error: "Only this organization's owner can delete it.",
    });
    mount();
    await press(/send confirmation code/i);
    await typeCode("123456");
    await press(/^delete this organization/i);

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Only this organization's owner can delete it.",
    );
    expect(screen.queryByText("Acme is deleted")).toBeNull();
    expect(deleteButton()).toBeInTheDocument();
  });
});
