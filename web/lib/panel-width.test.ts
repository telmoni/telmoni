import { describe, expect, it } from "vitest";

import {
  FILTERS_WIDTH_MAX,
  FILTERS_WIDTH_MIN,
  draggedPanelWidth,
  filtersBounds,
} from "./panel-width";

describe("a panel's edge dragged", () => {
  it("grows the way its edge faces, in whole pixels, between its bounds", () => {
    const left = { grows: "left" as const, min: 100, max: 800 };
    expect(draggedPanelWidth(640, 500, 400, left)).toBe(740);
    expect(draggedPanelWidth(640, 500, 560, left)).toBe(580);
    expect(draggedPanelWidth(640, 500, 1200, left)).toBe(100);
    expect(draggedPanelWidth(640, 500, -400, left)).toBe(800);
    const right = { grows: "right" as const, min: 128, max: 480 };
    expect(draggedPanelWidth(164, 300, 380, right)).toBe(244);
    expect(draggedPanelWidth(164, 300, 310.6, right)).toBe(175);
    expect(draggedPanelWidth(164, 300, 200, right)).toBe(128);
    expect(draggedPanelWidth(164, 300, 900, right)).toBe(480);
  });

  it("stands at the least where the bounds cross", () => {
    expect(draggedPanelWidth(400, 500, 0, { grows: "left", min: 420, max: 420 })).toBe(420);
    expect(draggedPanelWidth(400, 500, 0, { grows: "left", min: 500, max: 420 })).toBe(500);
  });
});

describe("the filters' bounds", () => {
  it("run from their least to their most, never past half the row", () => {
    expect(filtersBounds(1000)).toEqual({ grows: "right", min: FILTERS_WIDTH_MIN, max: FILTERS_WIDTH_MAX });
    expect(filtersBounds(700).max).toBe(350);
    // A row too narrow for even half: the least, so the edge still answers.
    expect(filtersBounds(200).max).toBe(FILTERS_WIDTH_MIN);
    // Not laid out yet: the whole range.
    expect(filtersBounds(0).max).toBe(FILTERS_WIDTH_MAX);
  });
});
