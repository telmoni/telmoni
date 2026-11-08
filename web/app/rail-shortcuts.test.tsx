// @vitest-environment jsdom
import { fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mockPush = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: mockPush }),
}));

import { RailShortcuts } from "@/components/rail-shortcuts";

describe("RailShortcuts", () => {
  beforeEach(() => {
    mockPush.mockClear();
    vi.restoreAllMocks();
  });

  it("navigates on matching digit key press", () => {
    render(
      <RailShortcuts
        shortcuts={[
          { key: "1", href: "https://github.com/telmoni/telmoni", newTab: true },
          { key: "2", href: "/docs" },
          { key: "3", href: "/plans" },
        ]}
      />,
    );

    fireEvent.keyDown(window, { key: "3" });
    expect(mockPush).toHaveBeenCalledWith("/plans");
  });

  it("opens off-origin or newTab links in a new window", () => {
    const openSpy = vi.spyOn(window, "open").mockImplementation(() => null);
    render(
      <RailShortcuts
        shortcuts={[
          { key: "1", href: "https://github.com/telmoni/telmoni", newTab: true },
        ]}
      />,
    );

    fireEvent.keyDown(window, { key: "1" });
    expect(openSpy).toHaveBeenCalledWith(
      "https://github.com/telmoni/telmoni",
      "_blank",
      "noreferrer",
    );
  });

  it("ignores key events when typing into an input", () => {
    render(
      <div>
        <input data-testid="input" />
        <RailShortcuts shortcuts={[{ key: "2", href: "/docs" }]} />
      </div>,
    );

    const input = document.querySelector("input")!;
    fireEvent.keyDown(input, { key: "2" });
    expect(mockPush).not.toHaveBeenCalled();
  });

  it("ignores key events when meta or ctrl key is held", () => {
    render(<RailShortcuts shortcuts={[{ key: "2", href: "/docs" }]} />);

    fireEvent.keyDown(window, { key: "2", metaKey: true });
    expect(mockPush).not.toHaveBeenCalled();
  });
});

// The rail swaps its primary links for its foot as the foot scrolls in; a
// page too short to scroll keeps both, or the primary links would be lost.
describe("the rail's swap", () => {
  let fire: (ratio: number) => void = () => {};
  let resize: () => void = () => {};

  afterEach(() => {
    vi.unstubAllGlobals();
    Reflect.deleteProperty(document.documentElement, "scrollHeight");
  });

  function mount(scrollHeight: number) {
    vi.stubGlobal(
      "IntersectionObserver",
      class {
        constructor(cb: (entries: { intersectionRatio: number }[]) => void) {
          fire = (ratio) => cb([{ intersectionRatio: ratio }]);
        }
        observe() {}
        disconnect() {}
      },
    );
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(cb: () => void) {
          resize = cb;
        }
        observe() {}
        disconnect() {}
      },
    );
    Object.defineProperty(document.documentElement, "scrollHeight", { configurable: true, value: scrollHeight });
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 700 });
    const { container } = render(
      <div data-public-rail>
        <div data-rail-footer />
        <RailShortcuts shortcuts={[]} />
      </div>,
    );
    return container.querySelector<HTMLElement>("[data-public-rail]")!;
  }

  it("swaps on a page that scrolls", () => {
    const rail = mount(2000);
    fire(0);
    expect(rail.dataset.footerVisible).toBe("false");
    fire(1);
    expect(rail.dataset.footerVisible).toBe("true");
  });

  it("keeps both blocks on a page too short to scroll, and swaps once it grows", () => {
    const rail = mount(700);
    fire(1);
    expect(rail.dataset.footerVisible).toBeUndefined();
    Object.defineProperty(document.documentElement, "scrollHeight", { configurable: true, value: 2000 });
    resize();
    // A stale ratio never hides the primary links; the observer's next reading does.
    expect(rail.dataset.footerVisible).toBeUndefined();
    fire(0);
    expect(rail.dataset.footerVisible).toBe("false");
  });

  it("drops the flag when a page shrinks until it no longer scrolls", () => {
    const rail = mount(2000);
    fire(0);
    expect(rail.dataset.footerVisible).toBe("false");
    Object.defineProperty(document.documentElement, "scrollHeight", { configurable: true, value: 700 });
    resize();
    expect(rail.dataset.footerVisible).toBeUndefined();
  });
});
