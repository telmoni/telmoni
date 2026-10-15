// @vitest-environment jsdom
import { fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const push = vi.fn();

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push }),
  usePathname: () => "/acme/web/traces",
}));
vi.mock("@/components/console-ui-context", () => ({
  useConsoleUi: () => ({ shortcutsOpen: false, setShortcutsOpen: vi.fn(), openSearch: vi.fn() }),
}));
vi.mock("@/components/page-action", () => ({ usePagePrimaryAction: () => null }));
vi.mock("@/lib/store", () => ({
  useActiveOrganization: () => ({ slug: "acme" }),
  useFlags: () => ({}),
}));

import { KeyboardShortcuts } from "@/components/keyboard-shortcuts";
import { LETTER_KEYS_OFF_CLASS } from "@/lib/accessibility";
import { offerPageSteps } from "@/lib/use-page-steps";

// `j` and `k` step through what a page offers — the trace list's peek —
// through the console's one key handler, after the pending `g`, which still
// takes `k` to API keys.
describe("KeyboardShortcuts - a page's steps", () => {
  const next = vi.fn();
  const previous = vi.fn();
  let takeBack: () => void = () => {};

  beforeEach(() => {
    vi.clearAllMocks();
    takeBack = offerPageSteps({ noun: "trace", next, previous });
  });

  afterEach(() => {
    takeBack();
    document.documentElement.classList.remove(LETTER_KEYS_OFF_CLASS);
  });

  it("steps forward on j and back on k", () => {
    render(<KeyboardShortcuts />);
    fireEvent.keyDown(document.body, { key: "j" });
    expect(next).toHaveBeenCalledTimes(1);
    expect(previous).not.toHaveBeenCalled();
    fireEvent.keyDown(document.body, { key: "k" });
    expect(previous).toHaveBeenCalledTimes(1);
    expect(next).toHaveBeenCalledTimes(1);
  });

  it("leaves g k to API keys", () => {
    render(<KeyboardShortcuts />);
    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "k" });
    expect(push).toHaveBeenCalledWith("/acme/web/api-keys");
    expect(previous).not.toHaveBeenCalled();
  });

  it("does nothing on a page that offers no steps, in a field, or with letter keys off", () => {
    takeBack();
    render(<KeyboardShortcuts />);
    fireEvent.keyDown(document.body, { key: "j" });
    expect(next).not.toHaveBeenCalled();

    takeBack = offerPageSteps({ noun: "trace", next, previous });
    const field = document.createElement("input");
    document.body.append(field);
    fireEvent.keyDown(field, { key: "j" });
    field.remove();
    expect(next).not.toHaveBeenCalled();

    document.documentElement.classList.add(LETTER_KEYS_OFF_CLASS);
    fireEvent.keyDown(document.body, { key: "j" });
    expect(next).not.toHaveBeenCalled();
  });
});
