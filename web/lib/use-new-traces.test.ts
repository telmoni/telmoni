// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import {
  LANDED_MAX,
  listDrawn,
  listIsEmpty,
  newTraces,
  tracesArrived,
  tracesShown,
  useNewTraces,
} from "./use-new-traces";

const fresh = { rows: [], empty: false, first: true };

describe("useNewTraces", () => {
  afterEach(() => {
    for (const project of ["project_1", "project_2"]) listDrawn(project, fresh);
  });

  it("counts each run once, per project, until the reader is shown them", () => {
    const { result } = renderHook(() => useNewTraces("project_1"));
    expect(result.current).toEqual({ count: 0, more: false });

    act(() => tracesArrived("project_1", ["a", "b"], false));
    act(() => tracesArrived("project_1", ["b", "c"], false));
    act(() => tracesArrived("project_2", ["z"], false));
    expect(result.current).toEqual({ count: 3, more: false });
    expect(newTraces("project_2")).toEqual({ count: 1, more: false });

    act(() => tracesShown("project_1"));
    expect(result.current).toEqual({ count: 0, more: false });
    expect(newTraces("project_2"), "another project's reader asked nothing").toEqual({
      count: 1,
      more: false,
    });
  });

  // A run exports its spans in batches, and the list has a row for it from
  // the first: the batches after are not new traces.
  it("does not count a run that is on the list already", () => {
    const { result } = renderHook(() => useNewTraces("project_1"));
    act(() => listDrawn("project_1", { rows: ["r1", "r2"], empty: false, first: true }));
    act(() => tracesArrived("project_1", ["r1"], false));
    expect(result.current).toEqual({ count: 0, more: false });
    act(() => tracesArrived("project_1", ["r2", "new"], false));
    expect(result.current).toEqual({ count: 1, more: false });
  });

  it("takes the rows a draw put on the list out of the count, and keeps the rest", () => {
    const { result } = renderHook(() => useNewTraces("project_1"));
    act(() => tracesArrived("project_1", ["a", "b", "c"], true));
    act(() => listDrawn("project_1", { rows: ["a", "b", "r1"], empty: false, first: false }));
    expect(result.current, "c was not drawn; the floor was").toEqual({ count: 1, more: false });

    act(() => listDrawn("project_1", { rows: ["r1"], empty: false, first: true }));
    expect(result.current, "a first draw stands before nothing").toEqual({
      count: 0,
      more: false,
    });
  });

  it("knows whether the list was drawn empty", () => {
    expect(listIsEmpty("project_1")).toBe(false);
    listDrawn("project_1", { rows: [], empty: true, first: true });
    expect(listIsEmpty("project_1")).toBe(true);
    listDrawn("project_1", { rows: ["a"], empty: false, first: false });
    expect(listIsEmpty("project_1")).toBe(false);
  });

  it("makes the count a floor once a nudge named only some, or the list keeps no more ids", () => {
    const { result } = renderHook(() => useNewTraces("project_1"));
    act(() => tracesArrived("project_1", ["a"], true));
    expect(result.current).toEqual({ count: 1, more: true });
    act(() => tracesShown("project_1"));

    const flood = Array.from({ length: LANDED_MAX + 1 }, (_, i) => `run_${i}`);
    act(() => tracesArrived("project_1", flood, false));
    expect(result.current).toEqual({ count: LANDED_MAX, more: true });
  });

  it("hands back the same snapshot until the count changes", () => {
    const { result, rerender } = renderHook(() => useNewTraces("project_1"));
    act(() => tracesArrived("project_1", ["a"], false));
    const first = result.current;
    rerender();
    expect(result.current).toBe(first);
    act(() => tracesArrived("project_1", ["a"], false));
    expect(result.current, "the same run again changes nothing").toBe(first);
    act(() => tracesArrived("project_1", ["b"], false));
    expect(result.current).not.toBe(first);
  });
});
