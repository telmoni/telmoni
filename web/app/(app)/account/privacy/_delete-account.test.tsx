// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Answer = { error?: string; ok?: boolean; deleted?: boolean };

const requestAccountDeletionCodeAction = vi.hoisted(() =>
  vi.fn<() => Promise<Answer>>(async () => ({ ok: true })),
);
const deleteAccountAction = vi.hoisted(() =>
  vi.fn<(code: string) => Promise<Answer>>(async () => ({ ok: true, deleted: true })),
);
vi.mock("./actions", () => ({ requestAccountDeletionCodeAction, deleteAccountAction }));

import { DeleteAccountForm } from "./_delete-account";

const replace = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  requestAccountDeletionCodeAction.mockResolvedValue({ ok: true });
  deleteAccountAction.mockResolvedValue({ ok: true, deleted: true });
  vi.stubGlobal("location", { replace });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const mount = (ownedOrganizations: readonly string[] = []) =>
  render(<DeleteAccountForm email="dev@example.com" ownedOrganizations={ownedOrganizations} />);

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

const deleteButton = () =>
  screen.getByRole("button", { name: /permanently delete my account/i });

describe("DeleteAccountForm", () => {
  it("offers the confirmation code straight away", () => {
    mount();
    expect(
      screen.getByRole("button", { name: /send confirmation code/i }),
    ).toBeEnabled();
  });

  // What goes with the account, named before anything is sent: the ones owned
  // alone are deleted with it, and one with anybody else in it blocks it.
  it("names the organizations the account owns before a code is sent", () => {
    mount(["Acme", "k@example.com"]);
    expect(document.body.textContent).toContain("You own Acme and k@example.com.");
    expect(requestAccountDeletionCodeAction).not.toHaveBeenCalled();
  });

  it("says nothing of owning when the account owns nothing it can name", () => {
    mount();
    expect(document.body.textContent).not.toMatch(/you own/i);
  });

  it("holds the delete shut until the six digits are in", async () => {
    mount();
    await press(/send confirmation code/i);
    expect(requestAccountDeletionCodeAction).toHaveBeenCalledTimes(1);

    expect(deleteButton()).toBeDisabled();
    await typeCode("12345");
    expect(deleteButton(), "five digits unlocked it").toBeDisabled();
    await typeCode("123456");
    expect(deleteButton()).toBeEnabled();
    expect(deleteAccountAction, "it deleted on a keystroke").not.toHaveBeenCalled();
  });

  // Auth's refusal names the organizations still in the way, and that is the
  // next step — so it is shown as it came, and nobody is signed out over it.
  it("shows what blocks the deletion, and signs nothing out", async () => {
    deleteAccountAction.mockResolvedValue({
      error:
        "conflict: you own organizations other people are in: Acme. Transfer ownership of each, or remove everyone else from it, then delete your account.",
    });
    mount(["Acme"]);
    await press(/send confirmation code/i);
    await typeCode("123456");
    await press(/permanently delete my account/i);

    expect(screen.getByRole("alert")).toHaveTextContent(/other people are in: Acme/);
    expect(screen.queryByText(/sorry to see you go/i)).toBeNull();
    expect(replace).not.toHaveBeenCalled();
  });

  // The sign-out route lands a session ended this way on the home page, not
  // on the provider's logout page (its route test pins that); the farewell says so.
  it("says goodbye and signs out to the home page once the account is gone", async () => {
    mount();
    await press(/send confirmation code/i);
    await typeCode("123456");
    await press(/permanently delete my account/i);

    expect(deleteAccountAction).toHaveBeenCalledWith("123456");
    expect(screen.getByText(/sorry to see you go/i)).toBeInTheDocument();
    expect(document.body.textContent).toMatch(/signing you out and taking you to the home page/i);

    await press(/sign out now/i);
    expect(replace).toHaveBeenCalledWith("/auth/logout");
  });
});
