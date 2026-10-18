// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The URL as Next's router holds it: changed by hand to stand for the router
// following a rewrite in a transition of its own, or a save's redirect.
let mockSearch = "";
vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(mockSearch),
}));

import { useLayout } from "@/components/traces/use-layout";
import { DEFAULT_LAYOUT, choiceOfLayout } from "@/lib/layout";
import type { TraceLayout } from "@/lib/server/data";
import { TRACE_PREFS_KEY, readPrefs } from "@/lib/trace-prefs";

const VIEW = "11111111-1111-4111-8111-111111111111";
const REVIEW: TraceLayout = {
  id: VIEW,
  kind: "layout",
  name: "Review",
  query: {
    depth: "steps",
    open: "none",
    timeline: false,
    pinned: ["answer"],
    filter: {},
    messages: false,
    scores: true,
  },
  columns: ["started", "agent", "status"],
  created_at: "2026-10-17T00:00:00Z",
  updated_at: "2026-10-17T00:00:00Z",
};
const PAGE = { layouts: [REVIEW], project: { view_id: null } };

const kept = () => readPrefs(window.localStorage.getItem(TRACE_PREFS_KEY)).layout;

/** The router following the last rewrite. */
function sync(rerender: () => void) {
  mockSearch = window.location.search.slice(1);
  rerender();
}

beforeEach(() => {
  mockSearch = "";
  window.localStorage.clear();
  window.history.replaceState(null, "", "/acme/web/sessions/conv-1?days=30");
  vi.spyOn(window.history, "replaceState");
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("useLayout", () => {
  it("opens on the link's saved layout, which becomes the browser's last choice", () => {
    mockSearch = `days=30&view=${VIEW}`;
    const { result } = renderHook(() => useLayout(PAGE));
    expect(result.current.source).toBe("link");
    expect(result.current.current?.name).toBe("Review");
    expect(result.current.isEdited).toBe(false);
    expect(kept()).toEqual(choiceOfLayout(REVIEW));
  });

  // ⚠ Next passes its own history state straight through: a rewrite that
  // handed it back would leave the router, and every reader of the URL, on
  // the old link, which would then be written back over the choice.
  it("rewrites the link's part of the URL with a fresh state, and stands until the router follows", () => {
    mockSearch = `days=30&view=${VIEW}`;
    const { result, rerender } = renderHook(() => useLayout(PAGE));
    // Customized, the link's part goes, the cut stands, and the layout it
    // began from is named as edited.
    act(() => result.current.setOptions({ timeline: true }));
    expect(window.history.replaceState).toHaveBeenLastCalledWith({}, "", "/acme/web/sessions/conv-1?days=30");
    // Before the router has followed: the choice, not the link.
    expect(mockSearch).toBe(`days=30&view=${VIEW}`);
    expect(result.current.options.timeline).toBe(true);
    expect(result.current.isEdited).toBe(true);
    expect(result.current.current?.name).toBe("Review");
    sync(rerender);
    expect(result.current.options.timeline).toBe(true);
    expect(result.current.source).toBe("browser");
    expect(kept()).toEqual({ view: VIEW, options: { ...choiceOfLayout(REVIEW).options, timeline: true } });

    // Reset goes back to it as saved, and the link names it again.
    act(() => result.current.reset());
    expect(result.current.isEdited).toBe(false);
    expect(window.history.replaceState).toHaveBeenLastCalledWith({}, "", `/acme/web/sessions/conv-1?days=30&view=${VIEW}`);
  });

  it("lets a link or a save's redirect take over from the choice applied", () => {
    const { result, rerender } = renderHook(() => useLayout(PAGE));
    act(() => result.current.setOptions({ depth: "calls" }));
    expect(result.current.isEdited).toBe(true);
    // Elsewhere than the rewrite went: a layout just saved, redirected to.
    mockSearch = `days=30&view=${VIEW}`;
    rerender();
    expect(result.current.current?.name).toBe("Review");
    expect(result.current.choice).toEqual(choiceOfLayout(REVIEW));
    expect(result.current.isEdited).toBe(false);
    expect(kept()).toEqual(choiceOfLayout(REVIEW));
  });

  it("opens in the default, customizes it, and goes back to it; a saved layout applied opens as saved", () => {
    const { result } = renderHook(() => useLayout(PAGE));
    expect(result.current.source).toBe("project");
    expect(result.current.options).toEqual(DEFAULT_LAYOUT);
    expect(result.current.current).toBeUndefined();
    act(() => result.current.setOptions({ depth: "steps" }));
    expect(result.current.isEdited).toBe(true);
    expect(kept()).toEqual({ view: null, options: { ...DEFAULT_LAYOUT, depth: "steps" } });
    act(() => result.current.reset());
    expect(result.current.isEdited).toBe(false);
    expect(result.current.options).toEqual(DEFAULT_LAYOUT);

    act(() => result.current.applyLayout(REVIEW));
    expect(result.current.current?.name).toBe("Review");
    expect(result.current.options.pinned).toEqual(["answer"]);
    expect(window.history.replaceState).toHaveBeenLastCalledWith({}, "", `/acme/web/sessions/conv-1?days=30&view=${VIEW}`);
  });

  it("opens in the project's default layout where the browser holds no choice", () => {
    const { result } = renderHook(() => useLayout({ layouts: [REVIEW], project: { view_id: VIEW } }));
    expect(result.current.source).toBe("project");
    expect(result.current.current?.name).toBe("Review");
    expect(result.current.isEdited).toBe(false);
  });
});
