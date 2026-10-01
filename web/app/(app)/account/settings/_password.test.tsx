// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const toastError = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({
  toast: { error: toastError, success: toastSuccess },
}));

const requestPasswordResetAction = vi.hoisted(() =>
  vi.fn<() => Promise<{ error: string | null }>>(async () => ({ error: null })),
);
vi.mock("./actions", () => ({ requestPasswordResetAction }));

import { PasswordReset } from "./_password";

beforeEach(() => {
  vi.clearAllMocks();
  requestPasswordResetAction.mockResolvedValue({ error: null });
});

const button = () => screen.getByRole("button");
const body = () => document.body.textContent ?? "";

describe("PasswordReset", () => {
  // Every test below that presses the button renders the `password` posture,
  // which is the only one that has a button.
  it("offers no field to type a password into", () => {
    render(<PasswordReset method="password" />);
    expect(screen.queryAllByRole("textbox")).toHaveLength(0);
    expect(document.querySelector('input[type="password"]')).toBeNull();
  });

  it("asks for the link and says to go and read the mail", async () => {
    render(<PasswordReset method="password" />);
    await act(async () => {
      fireEvent.click(button());
    });

    expect(requestPasswordResetAction).toHaveBeenCalledTimes(1);
    expect(toastSuccess).toHaveBeenCalledWith(
      "Check your email for a link to set a new password.",
    );
  });

  it("sends once, then stays sent", async () => {
    render(<PasswordReset method="password" />);
    await act(async () => {
      fireEvent.click(button());
    });
    expect(button()).toBeDisabled();
    expect(button()).toHaveTextContent(/link sent/i);

    await act(async () => {
      fireEvent.click(button());
    });
    expect(requestPasswordResetAction).toHaveBeenCalledTimes(1);
  });

  it("reports a refusal and lets them try again", async () => {
    requestPasswordResetAction.mockResolvedValue({
      error: "Too many requests — try again in an hour.",
    });
    render(<PasswordReset method="password" />);
    await act(async () => {
      fireEvent.click(button());
    });

    expect(toastError).toHaveBeenCalledWith(
      "Too many requests — try again in an hour.",
    );
    expect(toastSuccess).not.toHaveBeenCalled();
    expect(button()).toBeEnabled();
  });

  it("reports a network error without claiming anything was sent", async () => {
    requestPasswordResetAction.mockRejectedValue(new Error("offline"));
    render(<PasswordReset method="password" />);
    await act(async () => {
      fireEvent.click(button());
    });

    expect(toastError).toHaveBeenCalledWith("Network error. Please try again.");
    expect(toastSuccess).not.toHaveBeenCalled();
  });

  // A null method can be a provider sign-up, which is not offered a password.
  it("says one thing to somebody who signed in with a password", () => {
    render(<PasswordReset method="password" />);
    expect(body()).toMatch(/works once and expires within the hour/i);
    expect(body()).toMatch(/every signed-in session ends/i);
    expect(body()).not.toMatch(/google/i);
    expect(button()).toHaveTextContent(/password reset link/i);
  });

  // ⚠ The regression this guards is a button, not a sentence. A Google sign-up
  // offered a password link for months on the reasoning that spending it needs
  // the inbox — and the inbox IS the Google account, so it bought nothing and
  // minted a credential nobody asked for.
  //
  // Provider postures render empty rather than explaining themselves, and
  // `page.tsx` does not mount them at all — so the assertion is that nothing
  // whatsoever comes out. A passkey and an email link are not "providers" in
  // the OAuth sense and land here anyway: neither account has a password.
  // `null` is in the list because an unnamed method may well be one of these,
  // and a credential offered on a guess is the failure that matters.
  it("draws nothing at all for every account that has no password here", () => {
    for (const method of [
      "google",
      "microsoft",
      "passkey",
      "magic_link",
      "sso",
      null,
    ]) {
      const { container, unmount } = render(<PasswordReset method={method} />);
      expect(container).toBeEmptyDOMElement();
      expect(screen.queryByRole("button"), String(method)).toBeNull();
      expect(requestPasswordResetAction).not.toHaveBeenCalled();
      unmount();
    }
  });
});
