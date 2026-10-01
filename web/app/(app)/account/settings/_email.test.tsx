// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const refresh = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", () => ({ useRouter: () => ({ refresh }) }));

// Mocked only so the assertion below can prove this flow does NOT use toasts.
// The component does not import it today, and the day somebody copies the
// password control's shape into here, that test fails.
const toastError = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({ toast: { error: toastError, success: vi.fn() } }));

const requestEmailChangeAction = vi.hoisted(() =>
  vi.fn<
    (email: string) => Promise<{ error: string | null; email?: string }>
  >(async (email: string) => ({ error: null, email })),
);
const confirmEmailChangeAction = vi.hoisted(() =>
  vi.fn<
    (
      a: string,
      b: string,
    ) => Promise<{ error: string | null; email?: string }>
  >(async () => ({ error: null, email: "new@example.test" })),
);
vi.mock("./actions", () => ({
  requestEmailChangeAction,
  confirmEmailChangeAction,
}));

import { ChangeEmail } from "./_email";

const CURRENT = "k@example.test";

beforeEach(() => {
  vi.clearAllMocks();
  requestEmailChangeAction.mockImplementation(async (email: string) => ({
    error: null,
    email,
  }));
  confirmEmailChangeAction.mockResolvedValue({
    error: null,
    email: "new@example.test",
  });
});

const body = () => document.body.textContent ?? "";
const mount = (method: string | null) =>
  render(<ChangeEmail method={method} email={CURRENT} />);

const type = async (id: string, value: string) => {
  await act(async () => {
    fireEvent.change(document.querySelector(`#${id}`)!, { target: { value } });
  });
};
const press = async (name: RegExp) => {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name }));
  });
};

// Walk step one and land on the code step.
const reachCodeStep = async (address = "new@example.test") => {
  await type("new-email", address);
  await press(/send confirmation codes/i);
};

describe("ChangeEmail — who it offers anything to", () => {
  // ⚠ **EMPTY, not an explanation.** External providers manage address
  // changes directly, so each of these renders empty.
  // `page.tsx` no longer mounts this component for any of them, and the
  // component itself renders nothing if something else does, so the assertion
  // is that the container comes out empty.
  //
  // `sso` and `null` belong in the same list for different reasons: a directory
  // would assert the old address back over anything we set, and an unnamed
  // method may well be a provider sign-up. Both fail closed.
  it("draws nothing at all for every account whose provider holds the address", async () => {
    for (const method of [
      "google",
      "microsoft",
      "github",
      "passkey",
      "magic_link",
      "okta",
      "sso",
      null,
    ]) {
      const { container, unmount } = mount(method);
      expect(container, String(method)).toBeEmptyDOMElement();
      expect(screen.queryAllByRole("textbox"), String(method)).toHaveLength(0);
      expect(screen.queryAllByRole("button"), String(method)).toHaveLength(0);
      unmount();
    }
    expect(requestEmailChangeAction).not.toHaveBeenCalled();
  });

  it("offers the form only to an email-and-password account", () => {
    mount("password");
    expect(screen.getAllByRole("textbox")).toHaveLength(1);
    expect(
      screen.getByRole("button", { name: /send confirmation codes/i }),
    ).toBeInTheDocument();
  });
});

describe("ChangeEmail — asking for the codes", () => {
  it("will not send until the address looks like one", async () => {
    mount("password");
    const button = () =>
      screen.getByRole("button", { name: /send confirmation codes/i });
    expect(button()).toBeDisabled();

    for (const bad of ["nope", "a@b", "with space@x.test"]) {
      await type("new-email", bad);
      expect(button(), bad).toBeDisabled();
    }
    await type("new-email", "new@example.test");
    expect(button()).toBeEnabled();
  });

  // Case-insensitively: the stored address is lowercase, so one differing only
  // in case is the same address and sending would waste two emails.
  it("refuses to mail the address already on the account", async () => {
    mount("password");
    for (const same of [CURRENT, "K@Example.TEST"]) {
      await type("new-email", same);
      expect(
        screen.getByRole("button", { name: /send confirmation codes/i }),
        same,
      ).toBeDisabled();
    }
    expect(requestEmailChangeAction).not.toHaveBeenCalled();
  });

  // ⚠ The code step names the address the SERVER echoed, not the one that was
  // typed. Otherwise editing the field after sending would print the wrong
  // destination and confirm against an address the codes were not minted for.
  it("asks for one code per inbox, naming the address the server echoed", async () => {
    requestEmailChangeAction.mockResolvedValue({
      error: null,
      email: "new@example.test",
    });
    mount("password");
    await reachCodeStep("  New@Example.TEST  ");

    expect(requestEmailChangeAction).toHaveBeenCalledTimes(1);
    expect(requestEmailChangeAction).toHaveBeenCalledWith("New@Example.TEST");
    expect(body()).toMatch(/new@example\.test/);
    expect(body()).toMatch(new RegExp(CURRENT.replace(".", "\\.")));
    expect(document.querySelector("#email-code-new")).toHaveAttribute(
      "autoComplete",
      "one-time-code",
    );
  });

  it("reports a refusal inline, beside the field, and never as a toast", async () => {
    requestEmailChangeAction.mockResolvedValue({
      error: "That address is already in use.",
    });
    mount("password");
    await reachCodeStep();

    expect(screen.getByRole("alert")).toHaveTextContent(
      "That address is already in use.",
    );
    // Still on step one, so the address can be corrected in place.
    expect(document.querySelector("#new-email")).not.toBeNull();
    expect(toastError).not.toHaveBeenCalled();
  });

  it("reports a network error without claiming anything was sent", async () => {
    requestEmailChangeAction.mockRejectedValue(new Error("offline"));
    mount("password");
    await reachCodeStep();

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Network error. Try again.",
    );
    expect(document.querySelector("#new-email")).not.toBeNull();
  });
});

describe("ChangeEmail — spending the codes", () => {
  it("will not confirm until both codes are six digits", async () => {
    mount("password");
    await reachCodeStep();
    const button = () => screen.getByRole("button", { name: /change my email/i });

    expect(button()).toBeDisabled();
    await type("email-code-new", "222222");
    expect(button()).toBeDisabled();
    await type("email-code-current", "11111");
    expect(button()).toBeDisabled();
    await type("email-code-current", "111111");
    expect(button()).toBeEnabled();
  });

  it("keeps only digits, and only six", async () => {
    mount("password");
    await reachCodeStep();
    await type("email-code-new", "1a2b3c4d5e6f7");
    expect(document.querySelector<HTMLInputElement>("#email-code-new")!.value).toBe(
      "123456",
    );
  });

  // No address input on this step, so the address the codes were minted for
  // cannot drift out from under them.
  it("confirms with both codes and no address to drift", async () => {
    mount("password");
    await reachCodeStep();
    await type("email-code-new", "222222");
    await type("email-code-current", "111111");
    await press(/change my email/i);

    expect(confirmEmailChangeAction).toHaveBeenCalledWith("111111", "222222");
    expect(document.querySelector("#new-email")).toBeNull();
  });

  it("lets them ask again without starting over, and clears the dead codes", async () => {
    mount("password");
    await reachCodeStep();
    await type("email-code-new", "222222");
    await press(/resend codes/i);

    expect(requestEmailChangeAction).toHaveBeenCalledTimes(2);
    expect(requestEmailChangeAction).toHaveBeenLastCalledWith(
      "new@example.test",
    );
    expect(document.querySelector<HTMLInputElement>("#email-code-new")!.value).toBe(
      "",
    );
  });

  // The deletion flow has no equivalent and this one needs it: without a way
  // back, a typo'd address has no escape but a reload.
  it("lets them go back and fix a typo, keeping what they typed", async () => {
    mount("password");
    await reachCodeStep();
    await press(/use a different address/i);

    const input = document.querySelector<HTMLInputElement>("#new-email");
    expect(input).not.toBeNull();
    expect(input!.value).toBe("new@example.test");
    expect(document.querySelector("#email-code-new")).toBeNull();
  });

  it("leaves a wrong code on the code step, with the field still editable", async () => {
    confirmEmailChangeAction.mockResolvedValue({
      error: "invalid or expired confirmation code",
    });
    mount("password");
    await reachCodeStep();
    await type("email-code-new", "222222");
    await type("email-code-current", "111111");
    await press(/change my email/i);

    expect(screen.getByRole("alert")).toHaveTextContent(
      "invalid or expired confirmation code",
    );
    expect(document.querySelector("#email-code-new")).not.toBeNull();
    expect(toastError).not.toHaveBeenCalled();
  });

  // ⚠ No refresh: the session ended with the change, and the (app) layout
  // sends a dead session to /auth/logout. Refreshing would redirect the person
  // away mid-sentence — before they read which address the account now has, or
  // why they are signed out. This panel is the only place either is said.
  it("says the change landed and which address to use, without refreshing it away", async () => {
    mount("password");
    await reachCodeStep();
    await type("email-code-new", "222222");
    await type("email-code-current", "111111");
    await press(/change my email/i);

    expect(body()).toMatch(/now new@example\.test/);
    expect(body()).toMatch(/password has not changed/i);
    expect(screen.queryAllByRole("textbox")).toHaveLength(0);
    expect(refresh).not.toHaveBeenCalled();
  });

  // ⚠ The pin that keeps this honest. Auth revokes EVERY session on a
  // confirmed change, this browser's included, and refuses its bearer from
  // then on. Copy that said "your other devices" read as a courtesy and left
  // somebody to discover the sign-out as a malfunction.
  it("says this session ended too, and offers the way back in", async () => {
    mount("password");
    await reachCodeStep();
    await type("email-code-new", "222222");
    await type("email-code-current", "111111");
    await press(/change my email/i);

    expect(body()).toMatch(/this one included/i);
    expect(body()).not.toMatch(/other signed-in devices/i);
    expect(
      screen.getByRole("button", { name: /sign in again/i }),
    ).toBeInTheDocument();
  });

  // The change committed even when auth's answer does not say where it landed;
  // the address the codes were minted for is the one to name then.
  it("names the address the codes were sent to when auth's answer does not", async () => {
    confirmEmailChangeAction.mockResolvedValue({ error: null });
    mount("password");
    await reachCodeStep();
    await type("email-code-new", "222222");
    await type("email-code-current", "111111");
    await press(/change my email/i);

    expect(body()).toMatch(/now new@example\.test/);
    // The visible control, not the navigation: jsdom's location is not worth
    // fighting, and the button is the affordance that matters.
    expect(
      screen.getByRole("button", { name: /sign in again/i }),
    ).toBeInTheDocument();
  });
});
