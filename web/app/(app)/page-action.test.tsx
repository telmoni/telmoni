// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PageAction, PagePrimaryActionProvider, usePagePrimaryAction } from "@/components/page-action";

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
