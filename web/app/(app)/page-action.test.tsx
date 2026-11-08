// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PageAction, PagePrimaryActionProvider, usePagePrimaryAction } from "@/components/page-action";
import { setLetterKeys } from "@/lib/use-accessibility";

/** The key's side of the page's primary action, as the shortcuts run it. */
function PressC() {
  const action = usePagePrimaryAction();
  return (
    <button type="button" onClick={() => action?.run()}>
      Press C
    </button>
  );
}

describe("PageAction", () => {
  it("shows the primary shortcut without changing the button's accessible name", () => {
    const { rerender } = render(<PageAction primary="New alert">New alert</PageAction>);
    const button = screen.getByRole("button", { name: "New alert" });
    expect(button).toHaveAttribute("aria-keyshortcuts", "c");
    expect(button.querySelector("kbd")).toHaveTextContent("C");

    rerender(<PageAction primary="New alert" disabled>New alert</PageAction>);
    expect(button.querySelector("kbd")).toBeNull();
    expect(button).not.toHaveAttribute("aria-keyshortcuts");

    rerender(<PageAction>Secondary action</PageAction>);
    expect(screen.getByRole("button", { name: "Secondary action" }).querySelector("kbd")).toBeNull();
  });

  it("hides the hint when letter-key shortcuts are turned off", () => {
    render(<PageAction primary="New alert">New alert</PageAction>);
    try {
      act(() => setLetterKeys(false));
      const button = screen.getByRole("button", { name: "New alert" });
      expect(button.querySelector("kbd")).toBeNull();
      expect(button).not.toHaveAttribute("aria-keyshortcuts");
    } finally {
      act(() => setLetterKeys(true));
    }
  });

  it("keeps link actions as one link with a shortcut hint", () => {
    render(<PageAction primary="Plans" asChild><a href="/plans" target="_blank" rel="noreferrer">Plans</a></PageAction>);
    const link = screen.getByRole("link", { name: "Plans" });
    expect(link).toHaveAttribute("href", "/plans");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link.querySelector("kbd")).toHaveTextContent("C");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("is not run by its key under a panel that covers the page", () => {
    const made = vi.fn();
    render(
      <PagePrimaryActionProvider>
        <main id="console-main">
          <PageAction primary="New alert" onClick={made}>
            New alert
          </PageAction>
        </main>
        <PressC />
      </PagePrimaryActionProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Press C" }));
    expect(made).toHaveBeenCalledTimes(1);

    document.getElementById("console-main")?.setAttribute("inert", "");
    fireEvent.click(screen.getByRole("button", { name: "Press C" }));
    expect(made).toHaveBeenCalledTimes(1);
  });
});
