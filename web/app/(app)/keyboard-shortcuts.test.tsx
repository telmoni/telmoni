// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const push = vi.fn();
let mockSheetOpen = false;
let mockAction: { label: string; run: () => void } | null = null;

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push }),
  usePathname: () => "/acme/web/traces",
}));
vi.mock("@/components/console-ui-context", () => ({
  useConsoleUi: () => ({ shortcutsOpen: mockSheetOpen, setShortcutsOpen: vi.fn(), openSearch: vi.fn() }),
}));
vi.mock("@/components/page-action", () => ({ usePagePrimaryAction: () => mockAction }));
vi.mock("@/lib/store", () => ({
  useActiveOrganization: () => ({ slug: "acme" }),
  useFlags: () => ({}),
}));

import { KeyboardShortcuts } from "@/components/keyboard-shortcuts";
import { LETTER_KEYS_OFF_CLASS } from "@/lib/accessibility";
import { setPageCovered } from "@/lib/page-cover";
import { offerPageSteps } from "@/lib/use-page-steps";

// `j` and `k` step through what a page offers — the trace list's peek, a
// session's traces — and `o` opens the one stood on where the page offers
// that, through the console's one key handler, after the pending `g`, which
// still takes `k` to API keys and `o` to the Overview.
describe("KeyboardShortcuts - a page's steps", () => {
  const next = vi.fn();
  const previous = vi.fn();
  const open = vi.fn();
  let takeBack: () => void = () => {};

  beforeEach(() => {
    vi.clearAllMocks();
    takeBack = offerPageSteps({ noun: "trace", next, previous });
  });

  afterEach(() => {
    takeBack();
    setPageCovered(false);
    mockSheetOpen = false;
    mockAction = null;
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

  it("opens on o where the page offers it, and leaves g o to the Overview", () => {
    render(<KeyboardShortcuts />);
    fireEvent.keyDown(document.body, { key: "o" });
    expect(open).not.toHaveBeenCalled();

    takeBack();
    takeBack = offerPageSteps({ noun: "trace", next, previous, open });
    fireEvent.keyDown(document.body, { key: "o" });
    expect(open).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "o" });
    expect(push).toHaveBeenCalledWith("/acme/web");
    expect(open).toHaveBeenCalledTimes(1);
  });

  // ⚠ A panel over the page whole keeps its own steps and holds the rail's sequences off.
  it("steps under a panel that covers the page whole, where g leads nowhere", () => {
    setPageCovered(true);
    render(<KeyboardShortcuts />);
    fireEvent.keyDown(document.body, { key: "j" });
    expect(next).toHaveBeenCalledTimes(1);
    for (const key of ["g", "t", "g", "k"]) fireEvent.keyDown(document.body, { key });
    expect(push).not.toHaveBeenCalled();
    expect(previous).not.toHaveBeenCalled();
  });

  it("lists neither the sequences nor the page's action while a panel covers the page", () => {
    mockSheetOpen = true;
    mockAction = { label: "New alert", run: vi.fn() };
    setPageCovered(true);
    render(<KeyboardShortcuts />);
    const sheet = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
    expect(within(sheet).getByText("Next trace")).toBeInTheDocument();
    expect(within(sheet).getByText("Search")).toBeInTheDocument();
    expect(within(sheet).queryByText("Traces")).toBeNull();
    expect(within(sheet).queryByText("New alert")).toBeNull();

    act(() => setPageCovered(false));
    expect(within(sheet).getByText("Traces")).toBeInTheDocument();
    expect(within(sheet).getByText("New alert")).toBeInTheDocument();
  });

  it("leaves a form in a panel its own keys", () => {
    // As the side panel draws itself, over the page and not modal.
    const panel = document.createElement("div");
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-modal", "false");
    panel.innerHTML = '<form><button type="button">Error</button></form>';
    document.body.append(panel);
    render(<KeyboardShortcuts />);
    const control = panel.querySelector("button")!;
    for (const key of ["j", "g", "t"]) fireEvent.keyDown(control, { key });
    panel.remove();
    expect(next).not.toHaveBeenCalled();
    expect(push).not.toHaveBeenCalled();
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
