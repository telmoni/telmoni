// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(),
  usePathname: () => "/acme/web/traces",
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn(), prefetch: vi.fn() }),
  unstable_rethrow: () => {},
}));
const { go, storedIdAction } = vi.hoisted(() => ({ go: vi.fn(), storedIdAction: vi.fn() }));
vi.mock("@/components/traces/navigate", () => ({
  useListNavigation: () => ({ go, pending: false }),
}));
vi.mock("@/app/(app)/[organization]/[project]/traces/actions", () => ({ storedIdAction }));

import { FacetsPanel } from "@/components/traces/facets";
import { TooltipProvider } from "@/components/ui/tooltip";
import { FILTERS_WIDTH_DEFAULT, FILTERS_WIDTH_MAX, FILTERS_WIDTH_STEP } from "@/lib/panel-width";
import type { Facets, ListQuery } from "@/lib/server/entities/traces";
import { TRACE_PREFS_KEY } from "@/lib/trace-prefs";
import { FILTERS, FILTER_LABEL, PRIMARY_FILTERS, type ListName } from "@/lib/traces";

/** Every filter with one value among the window's traces. */
const FACETS = {
  days: 7,
  ...Object.fromEntries(FILTERS.map((filter) => [filter, { values: [{ value: `${filter}-1`, count: 3 }], more: false }])),
} as Facets;

/** The sections the panel draws, by their headings, a ticked one's count beside its name. */
const sections = () =>
  FILTERS.filter(
    (filter) => screen.queryByRole("button", { name: new RegExp(`^${FILTER_LABEL[filter]}(\\s*\\d+)?$`) }) !== null,
  );

/** The filters dragged 80px wider than their own width. */
const DRAGGED = FILTERS_WIDTH_DEFAULT + 80;

/** The width the edge reports the filters at. */
const now = () => Number(screen.getByRole("separator", { name: "Resize the filters" }).getAttribute("aria-valuenow"));

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

function panel(query: ListQuery<string> = {}, facets: Facets | null = null, list: ListName = "traces") {
  return render(
    <TooltipProvider>
      <FacetsPanel
        list={list}
        organization="acme"
        slug="web"
        projectId="project_1"
        query={query}
        facets={facets}
      />
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
    expect(aside.style.getPropertyValue("--filters-width")).toBe(`${DRAGGED}px`);

    fireEvent.pointerUp(edge, { pointerId: 1, clientX: 380 });
    expect(aside.style.getPropertyValue("--filters-width")).toBe(`${DRAGGED}px`);
    expect(edge).toHaveAttribute("aria-valuenow", String(DRAGGED));

    fireEvent.doubleClick(edge);
    expect(now()).toBe(FILTERS_WIDTH_DEFAULT);
    expect(aside.style.getPropertyValue("--filters-width")).toBe("");
  });

  it("never passes half the row, nor its least", () => {
    panel();
    const edge = screen.getByRole("separator", { name: "Resize the filters" });
    fireEvent.pointerDown(edge, { button: 0, isPrimary: true, pointerId: 2, clientX: 300 });
    fireEvent.pointerMove(edge, { pointerId: 2, clientX: 2000 });
    expect(edge).toHaveAttribute("aria-valuenow", String(FILTERS_WIDTH_MAX));
    fireEvent.pointerMove(edge, { pointerId: 2, clientX: -2000 });
    expect(edge).toHaveAttribute("aria-valuenow", String(FILTERS_WIDTH_DEFAULT));
    // Another pointer's move is not this drag's.
    fireEvent.pointerMove(edge, { pointerId: 3, clientX: 500 });
    expect(edge).toHaveAttribute("aria-valuenow", String(FILTERS_WIDTH_DEFAULT));
    fireEvent.pointerCancel(edge, { pointerId: 2 });
    expect(now()).toBe(FILTERS_WIDTH_DEFAULT);
    // Back to its own: the width outlives the panel, as a page left and come back to.
    fireEvent.doubleClick(edge);
  });

  it("takes the arrow keys, Home and End within its bounds, and Enter for its own width", () => {
    panel();
    const edge = screen.getByRole("separator", { name: "Resize the filters" });
    fireEvent.keyDown(edge, { key: "ArrowRight" });
    expect(now()).toBe(FILTERS_WIDTH_DEFAULT + FILTERS_WIDTH_STEP);
    fireEvent.keyDown(edge, { key: "ArrowLeft" });
    expect(now()).toBe(FILTERS_WIDTH_DEFAULT);
    fireEvent.keyDown(edge, { key: "End" });
    expect(now()).toBe(FILTERS_WIDTH_MAX);
    fireEvent.keyDown(edge, { key: "ArrowRight" });
    expect(now()).toBe(FILTERS_WIDTH_MAX);
    fireEvent.keyDown(edge, { key: "Home" });
    expect(now()).toBe(FILTERS_WIDTH_DEFAULT);
    fireEvent.keyDown(edge, { key: "Enter" });
    expect(now()).toBe(FILTERS_WIDTH_DEFAULT);
    expect(screen.getByRole("complementary", { name: "Filters" }).style.getPropertyValue("--filters-width")).toBe("");
  });

  it("opens a page come back to at the width it was left at, and a reload at its own", async () => {
    const left = panel();
    fireEvent.keyDown(screen.getByRole("separator", { name: "Resize the filters" }), { key: "End" });
    expect(now()).toBe(FILTERS_WIDTH_MAX);
    left.unmount();

    const back = panel();
    expect(now()).toBe(FILTERS_WIDTH_MAX);
    // Back to its own, so the tests after this one open at it.
    fireEvent.keyDown(screen.getByRole("separator", { name: "Resize the filters" }), { key: "Enter" });
    back.unmount();

    // A reload loads the modules afresh; an older build's saved width is not read either.
    vi.resetModules();
    window.localStorage.setItem(TRACE_PREFS_KEY, JSON.stringify({ filtersWidth: 244 }));
    const { FacetsPanel: Reloaded } = await import("@/components/traces/facets");
    const { TooltipProvider: Provider } = await import("@/components/ui/tooltip");
    render(
      <Provider>
        <Reloaded list="traces" organization="acme" slug="web" projectId="project_1" query={{}} facets={null} />
      </Provider>,
    );
    expect(screen.getByRole("complementary", { name: "Filters" }).style.getPropertyValue("--filters-width")).toBe("");
    expect(now()).toBe(FILTERS_WIDTH_DEFAULT);
  });

  it("is not there while the filters are hidden", () => {
    window.localStorage.setItem(TRACE_PREFS_KEY, JSON.stringify({ filters: false }));
    panel();
    expect(screen.queryByRole("separator")).toBeNull();
    expect(screen.getByRole("button", { name: "Show the filters" })).toBeInTheDocument();
  });
});

describe("the score filter", () => {
  beforeEach(() => {
    window.localStorage.removeItem(TRACE_PREFS_KEY);
    go.mockClear();
  });

  it("narrows the list to a score's name within the range given", () => {
    panel();
    fireEvent.click(screen.getByRole("button", { name: "More filters" }));
    fireEvent.click(screen.getByRole("button", { name: "Score" }));
    const apply = screen.getByRole("button", { name: "Apply" });
    expect(apply).toBeDisabled();
    fireEvent.change(screen.getByLabelText("A score's name"), { target: { value: " relevance " } });
    fireEvent.change(screen.getByLabelText("At least"), { target: { value: "0.8" } });
    fireEvent.change(screen.getByLabelText("At most"), { target: { value: "0.5" } });
    expect(apply).toBeDisabled();
    fireEvent.change(screen.getByLabelText("At most"), { target: { value: "1" } });
    fireEvent.click(apply);
    expect(go).toHaveBeenCalledTimes(1);
    const href = new URL(String(go.mock.calls[0]?.[0]), "https://x");
    expect([href.searchParams.get("score"), href.searchParams.get("score_min"), href.searchParams.get("score_max")]).toEqual([
      "relevance",
      "0.8",
      "1",
    ]);
  });

  it("says whose score counts on each list: a session's own, or one of its traces'", () => {
    const traces = panel({ score: "csat" });
    expect(screen.getByText(/^Rows with a score of this name/)).toBeInTheDocument();
    traces.unmount();
    panel({ score: "csat" }, null, "sessions");
    expect(
      screen.getByText(/^Sessions with a score of this name, their own or one of their traces'/),
    ).toBeInTheDocument();
  });
});

describe("the filters the panel opens with", () => {
  beforeEach(() => {
    window.localStorage.removeItem(TRACE_PREFS_KEY);
  });

  it("shows the few most narrowing starts from, and the rest once More is opened, as the browser keeps it", () => {
    const first = panel({}, FACETS);
    expect(sections()).toEqual(PRIMARY_FILTERS);
    expect(screen.queryByRole("button", { name: "Score" })).toBeNull();
    const more = screen.getByRole("button", { name: "More filters" });
    expect(more).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(more);
    expect(sections()).toEqual(FILTERS);
    expect(screen.getByRole("button", { name: "Score" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Fewer filters" })).toHaveAttribute("aria-expanded", "true");
    first.unmount();

    // The page opened again: the panel stands as it was left.
    panel({}, FACETS);
    expect(sections()).toEqual(FILTERS);
    fireEvent.click(screen.getByRole("button", { name: "Fewer filters" }));
    expect(sections()).toEqual(PRIMARY_FILTERS);
  });

  it("keeps a filter in force in view, so it is cleared where it was set", () => {
    panel({ tool: ["search_docs"], score: "helpful" }, FACETS);
    expect(sections()).toEqual([...PRIMARY_FILTERS.slice(0, 4), "tool", "environment"]);
    expect(screen.getByRole("button", { name: /^Score\s*1$/ })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Clear the tool filter" })).toBeInTheDocument();
  });

  it("searches every filter, the ones behind More among them", () => {
    panel({}, FACETS);
    fireEvent.change(screen.getByLabelText("Search filters"), { target: { value: "prompt" } });
    expect(sections()).toEqual(["prompt", "prompt_version"]);
    expect(screen.queryByRole("button", { name: "More filters" })).toBeNull();
  });
});

describe("an id typed as the application sends it", () => {
  const STORED = "9f86d081884c7d659a2feaa0c55ad015";
  const SENT = "ada@example.com";

  beforeEach(() => {
    window.localStorage.setItem(TRACE_PREFS_KEY, JSON.stringify({ moreFilters: true }));
    go.mockClear();
    storedIdAction.mockReset();
  });

  /** The user filter's box, opened on the id as sent. */
  function typed(query: ListQuery<string> = {}) {
    panel(query, FACETS);
    // Open already where a user is ticked, its count beside its name.
    if (!screen.queryByRole("radio", { name: "Your id" })) {
      fireEvent.click(screen.getByRole("button", { name: /^User(\s*\d+)?$/ }));
    }
    fireEvent.click(screen.getByRole("radio", { name: "Your id" }));
    const box = screen.getByLabelText("The user's id, as your app sends it");
    fireEvent.change(box, { target: { value: SENT } });
    return box;
  }

  it("is looked up as the project stores it, and the list narrowed to that, never to the id", async () => {
    storedIdAction.mockResolvedValue({ ok: true, storedId: STORED });
    const box = typed();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Find" }));
    });
    expect(storedIdAction).toHaveBeenCalledTimes(1);
    expect(storedIdAction).toHaveBeenCalledWith("project_1", "user", SENT);
    expect(go).toHaveBeenCalledTimes(1);
    const href = String(go.mock.calls[0]?.[0]);
    expect(new URL(href, "https://x").searchParams.getAll("user")).toEqual([STORED]);
    expect(decodeURIComponent(href)).not.toContain(SENT);
    expect(box).toHaveValue("");
  });

  it("stays ticked where the stored id already narrows the list, and says a refusal without narrowing", async () => {
    storedIdAction.mockResolvedValueOnce({ ok: true, storedId: STORED });
    typed({ user: [STORED] });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Find" }));
    });
    expect(go).not.toHaveBeenCalled();

    storedIdAction.mockResolvedValueOnce({ ok: false, error: "Telemetry is unreachable. Try again." });
    fireEvent.change(screen.getByLabelText("The user's id, as your app sends it"), { target: { value: SENT } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Find" }));
    });
    expect(storedIdAction).toHaveBeenCalledTimes(2);
    expect(go).not.toHaveBeenCalled();
  });

  it("types a tag as key:value, with no lookup", () => {
    panel({}, FACETS);
    fireEvent.click(screen.getByRole("button", { name: "Tag" }));
    fireEvent.click(screen.getByRole("radio", { name: "Text" }));
    const add = screen.getByRole("button", { name: "Add" });
    fireEvent.change(screen.getByLabelText("A tag, as key:value"), { target: { value: "repo" } });
    expect(add).toBeDisabled();
    fireEvent.change(screen.getByLabelText("A tag, as key:value"), { target: { value: " repo:telmoni " } });
    fireEvent.click(add);
    expect(storedIdAction).not.toHaveBeenCalled();
    expect(new URL(String(go.mock.calls[0]?.[0]), "https://x").searchParams.getAll("tag")).toEqual(["repo:telmoni"]);
  });
});
