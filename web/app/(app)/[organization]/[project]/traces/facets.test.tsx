// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(),
  usePathname: () => "/acme/web/traces",
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn(), prefetch: vi.fn() }),
}));
vi.mock("@/components/traces/navigate", () => ({
  useListNavigation: () => ({ go: vi.fn(), pending: false }),
}));

import { FacetsPanel } from "@/components/traces/facets";
import { TooltipProvider } from "@/components/ui/tooltip";
import { FILTERS_WIDTH_DEFAULT, FILTERS_WIDTH_MAX, FILTERS_WIDTH_MIN, FILTERS_WIDTH_STEP } from "@/lib/panel-width";
import { TRACE_PREFS_KEY, readPrefs } from "@/lib/trace-prefs";

/** What the browser keeps of the filters' width. */
const kept = () => readPrefs(window.localStorage.getItem(TRACE_PREFS_KEY)).filtersWidth;

/** jsdom has no pointer events: one that carries what the edge reads. */
class FakePointerEvent extends MouseEvent {
  pointerId: number;
  isPrimary: boolean;
  constructor(type: string, init: PointerEventInit = {}) {
    super(type, init);
    this.pointerId = init.pointerId ?? 1;
    this.isPrimary = init.isPrimary ?? true;
  }
}

function panel() {
  return render(
    <TooltipProvider>
      <FacetsPanel list="traces" organization="acme" slug="web" query={{}} facets={null} />
    </TooltipProvider>,
  );
}

describe("the filters' edge", () => {
  beforeEach(() => {
    window.localStorage.removeItem(TRACE_PREFS_KEY);
    vi.stubGlobal("PointerEvent", FakePointerEvent);
    HTMLElement.prototype.setPointerCapture = vi.fn();
    HTMLElement.prototype.releasePointerCapture = vi.fn();
    // jsdom lays nothing out: the filters stand at their own width in a 1,000px row.
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const width = this.tagName === "ASIDE" ? FILTERS_WIDTH_DEFAULT : 1000;
      return { x: 0, y: 0, top: 0, left: 0, bottom: 0, right: width, width, height: 0, toJSON: () => ({}) } as DOMRect;
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("follows a drag rightwards, keeps the width it ends at, and lets it go on a double click", () => {
    panel();
    const edge = screen.getByRole("separator", { name: "Resize the filters" });
    const aside = screen.getByRole("complementary", { name: "Filters" });
    expect(edge).toHaveAttribute("aria-valuenow", String(FILTERS_WIDTH_DEFAULT));
    expect(aside.style.getPropertyValue("--filters-width")).toBe("");

    fireEvent.pointerDown(edge, { button: 0, isPrimary: true, pointerId: 1, clientX: 300 });
    fireEvent.pointerMove(edge, { pointerId: 1, clientX: 380 });
    expect(aside.style.getPropertyValue("--filters-width")).toBe("244px");
    // Followed, not yet kept: one write a drag.
    expect(kept()).toBeNull();

    fireEvent.pointerUp(edge, { pointerId: 1, clientX: 380 });
    expect(kept()).toBe(244);
    expect(aside.style.getPropertyValue("--filters-width")).toBe("244px");
    expect(edge).toHaveAttribute("aria-valuenow", "244");

    fireEvent.doubleClick(edge);
    expect(kept()).toBeNull();
    expect(aside.style.getPropertyValue("--filters-width")).toBe("");
  });

  it("never passes half the row, nor its least", () => {
    panel();
    const edge = screen.getByRole("separator", { name: "Resize the filters" });
    fireEvent.pointerDown(edge, { button: 0, isPrimary: true, pointerId: 2, clientX: 300 });
    fireEvent.pointerMove(edge, { pointerId: 2, clientX: 2000 });
    expect(edge).toHaveAttribute("aria-valuenow", String(FILTERS_WIDTH_MAX));
    fireEvent.pointerMove(edge, { pointerId: 2, clientX: -2000 });
    expect(edge).toHaveAttribute("aria-valuenow", String(FILTERS_WIDTH_MIN));
    // Another pointer's move is not this drag's.
    fireEvent.pointerMove(edge, { pointerId: 3, clientX: 500 });
    expect(edge).toHaveAttribute("aria-valuenow", String(FILTERS_WIDTH_MIN));
    fireEvent.pointerCancel(edge, { pointerId: 2 });
    expect(kept()).toBe(FILTERS_WIDTH_MIN);
  });

  it("takes the arrow keys, Home and End within its bounds, and Enter for its own width", () => {
    panel();
    const edge = screen.getByRole("separator", { name: "Resize the filters" });
    fireEvent.keyDown(edge, { key: "ArrowRight" });
    expect(kept()).toBe(FILTERS_WIDTH_DEFAULT + FILTERS_WIDTH_STEP);
    fireEvent.keyDown(edge, { key: "ArrowLeft" });
    expect(kept()).toBe(FILTERS_WIDTH_DEFAULT);
    fireEvent.keyDown(edge, { key: "End" });
    expect(kept()).toBe(FILTERS_WIDTH_MAX);
    fireEvent.keyDown(edge, { key: "ArrowRight" });
    expect(kept()).toBe(FILTERS_WIDTH_MAX);
    fireEvent.keyDown(edge, { key: "Home" });
    expect(kept()).toBe(FILTERS_WIDTH_MIN);
    fireEvent.keyDown(edge, { key: "Enter" });
    expect(kept()).toBeNull();
    expect(edge).toHaveAttribute("aria-valuenow", String(FILTERS_WIDTH_DEFAULT));
  });

  it("stands at the width the browser kept, held to the filters' range", () => {
    window.localStorage.setItem(TRACE_PREFS_KEY, JSON.stringify({ filtersWidth: 244 }));
    const { unmount } = panel();
    expect(screen.getByRole("complementary", { name: "Filters" }).style.getPropertyValue("--filters-width")).toBe("244px");
    expect(screen.getByRole("separator", { name: "Resize the filters" })).toHaveAttribute("aria-valuenow", "244");
    unmount();
    window.localStorage.setItem(TRACE_PREFS_KEY, JSON.stringify({ filtersWidth: 9000 }));
    panel();
    expect(screen.getByRole("complementary", { name: "Filters" }).style.getPropertyValue("--filters-width")).toBe("");
  });

  it("is not there while the filters are hidden", () => {
    window.localStorage.setItem(TRACE_PREFS_KEY, JSON.stringify({ filters: false }));
    panel();
    expect(screen.queryByRole("separator")).toBeNull();
    expect(screen.getByRole("button", { name: "Show the filters" })).toBeInTheDocument();
  });
});
