// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { SkipLink } from "@/components/skip-link";

function page(covered: boolean) {
  render(
    <>
      <SkipLink />
      <main id="console-main" tabIndex={-1} />
      {covered && <div data-slot="side-panel" role="dialog" tabIndex={-1} />}
    </>,
  );
  const main = document.getElementById("console-main")!;
  main.inert = covered;
  return screen.getByRole("link", { name: "Skip to content" });
}

describe("SkipLink", () => {
  // jsdom follows no fragment, so the click's cancel stands for no history entry.
  it("moves the focus to the page without following the fragment", () => {
    expect(fireEvent.click(page(false))).toBe(false);
    expect(document.activeElement?.id).toBe("console-main");
  });

  it("moves the focus to the panel over the page whole, the page out of reach", () => {
    expect(fireEvent.click(page(true))).toBe(false);
    expect(document.activeElement?.getAttribute("data-slot")).toBe("side-panel");
  });

  it("leaves a click asking for another tab or window to the link", () => {
    expect(fireEvent.click(page(false), { ctrlKey: true })).toBe(true);
    expect(document.activeElement?.id).not.toBe("console-main");
  });
});
