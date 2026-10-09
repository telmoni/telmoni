// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  AGENT_WINDOW_STORAGE_KEY,
  WINDOW_EDGE_PX,
  WINDOW_MIN,
  clampRect,
  defaultRect,
  floatingRect,
  fullRect,
  isCompact,
  moveRect,
  readWindowLayout,
  resizeRect,
  writeWindowLayout,
} from "./window";

const SCREEN = { width: 1440, height: 900 };

afterEach(() => {
  vi.restoreAllMocks();
  window.localStorage.clear();
});

describe("isCompact", () => {
  it("fills a screen too narrow or too short to float on", () => {
    expect(isCompact({ width: 639, height: 900 })).toBe(true);
    expect(isCompact({ width: 640, height: 900 })).toBe(false);
    expect(isCompact({ width: 1440, height: 479 })).toBe(true);
    expect(isCompact({ width: 1440, height: 480 })).toBe(false);
  });
});

describe("defaultRect", () => {
  it("opens at the right, under the header, at its own size", () => {
    expect(defaultRect(SCREEN)).toEqual({ x: 946, y: 74, width: 480, height: 720 });
  });

  it("opens under a header a banner has pushed down", () => {
    expect(defaultRect(SCREEN, 100)).toEqual({ x: 946, y: 114, width: 480, height: 720 });
  });

  it("is no taller than the screen allows, and no shorter than the minimum", () => {
    expect(defaultRect({ width: 1280, height: 700 })).toEqual({ x: 786, y: 74, width: 480, height: 612 });
    expect(defaultRect({ width: 1280, height: 500 })).toEqual({
      x: 786,
      y: 74,
      width: 480,
      height: WINDOW_MIN.height,
    });
  });
});

describe("fullRect", () => {
  it("keeps the console's edge padding all round", () => {
    expect(fullRect(SCREEN)).toEqual({
      x: WINDOW_EDGE_PX,
      y: WINDOW_EDGE_PX,
      width: SCREEN.width - 2 * WINDOW_EDGE_PX,
      height: SCREEN.height - 2 * WINDOW_EDGE_PX,
    });
  });
});

describe("clampRect", () => {
  it("brings a window back on screen", () => {
    expect(clampRect({ x: 1300, y: 800, width: 480, height: 720 }, SCREEN)).toEqual({
      x: 960,
      y: 180,
      width: 480,
      height: 720,
    });
  });

  it("shrinks a window larger than the screen to it", () => {
    expect(clampRect({ x: -50, y: -50, width: 2000, height: 1200 }, SCREEN)).toEqual({
      x: 0,
      y: 0,
      width: 1440,
      height: 900,
    });
  });

  it("grows a window to the minimum, or to the screen where that is smaller", () => {
    expect(clampRect({ x: 10, y: 10, width: 100, height: 100 }, SCREEN)).toEqual({
      x: 10,
      y: 10,
      width: WINDOW_MIN.width,
      height: WINDOW_MIN.height,
    });
    expect(clampRect({ x: 10, y: 10, width: 480, height: 720 }, { width: 300, height: 300 })).toEqual({
      x: 0,
      y: 0,
      width: 300,
      height: 300,
    });
  });
});

describe("floatingRect", () => {
  it("opens where it first does until it has been moved, then where it was left", () => {
    expect(floatingRect({ mode: "float", rect: null }, SCREEN)).toEqual(defaultRect(SCREEN));
    const left = { x: 100, y: 120, width: 500, height: 600 };
    expect(floatingRect({ mode: "float", rect: left }, SCREEN)).toEqual(left);
    expect(floatingRect({ mode: "float", rect: left }, { width: 800, height: 700 })).toEqual({
      x: 100,
      y: 100,
      width: 500,
      height: 600,
    });
  });
});

describe("moveRect", () => {
  const start = { x: 946, y: 74, width: 480, height: 720 };

  it("follows the pointer, keeping its size", () => {
    expect(moveRect(start, -400, 50, SCREEN)).toEqual({ x: 546, y: 124, width: 480, height: 720 });
  });

  it("stops at the screen's edges", () => {
    expect(moveRect(start, 1000, 0, SCREEN).x).toBe(960);
    expect(moveRect(start, 0, -500, SCREEN).y).toBe(0);
    expect(moveRect(start, -5000, 5000, SCREEN)).toEqual({ x: 0, y: 180, width: 480, height: 720 });
  });
});

describe("resizeRect", () => {
  const start = { x: 500, y: 100, width: 480, height: 600 };

  it("sizes from the right and the bottom, the left and top holding", () => {
    expect(resizeRect(start, "e", 100, 0, SCREEN)).toEqual({ ...start, width: 580 });
    expect(resizeRect(start, "s", 0, 50, SCREEN)).toEqual({ ...start, height: 650 });
    expect(resizeRect(start, "se", 40, 30, SCREEN)).toEqual({ ...start, width: 520, height: 630 });
  });

  it("sizes from the left and the top, the right and bottom holding", () => {
    expect(resizeRect(start, "w", -100, 0, SCREEN)).toEqual({ ...start, x: 400, width: 580 });
    expect(resizeRect(start, "n", 0, -50, SCREEN)).toEqual({ ...start, y: 50, height: 650 });
    expect(resizeRect(start, "nw", -20, -20, SCREEN)).toEqual({ x: 480, y: 80, width: 500, height: 620 });
    expect(resizeRect(start, "ne", 10, -10, SCREEN)).toEqual({ x: 500, y: 90, width: 490, height: 610 });
    expect(resizeRect(start, "sw", -10, 10, SCREEN)).toEqual({ x: 490, y: 100, width: 490, height: 610 });
  });

  it("stops at the minimum without sliding", () => {
    expect(resizeRect(start, "w", 200, 0, SCREEN)).toEqual({ ...start, x: 620, width: WINDOW_MIN.width });
    expect(resizeRect(start, "n", 0, 400, SCREEN)).toEqual({ ...start, y: 280, height: WINDOW_MIN.height });
    expect(resizeRect(start, "e", -400, 0, SCREEN).width).toBe(WINDOW_MIN.width);
  });

  it("stops at the screen's edges", () => {
    expect(resizeRect(start, "e", 2000, 0, SCREEN)).toEqual({ ...start, width: 940 });
    expect(resizeRect(start, "w", -1000, 0, SCREEN)).toEqual({ ...start, x: 0, width: 980 });
    expect(resizeRect(start, "s", 0, 1000, SCREEN)).toEqual({ ...start, height: 800 });
    expect(resizeRect(start, "n", 0, -500, SCREEN)).toEqual({ ...start, y: 0, height: 700 });
  });
});

describe("the stored layout", () => {
  it("floats where it first opens when nothing is stored", () => {
    expect(readWindowLayout()).toEqual({ mode: "float", rect: null });
  });

  it("keeps the mode and a whole-pixel rect", () => {
    writeWindowLayout({ mode: "full", rect: { x: 10.4, y: 20.6, width: 400.5, height: 500 } });
    expect(readWindowLayout()).toEqual({
      mode: "full",
      rect: { x: 10, y: 21, width: 401, height: 500 },
    });
  });

  it("ignores what this build cannot read", () => {
    for (const raw of [
      "{nope",
      '{"mode":"sideways","rect":null}',
      '{"mode":"float","rect":{"x":1,"y":2,"width":0,"height":5}}',
      '{"mode":"float","rect":{"x":"1","y":2,"width":3,"height":5}}',
      "null",
    ]) {
      window.localStorage.setItem(AGENT_WINDOW_STORAGE_KEY, raw);
      expect(readWindowLayout(), raw).toEqual({ mode: "float", rect: null });
    }
  });

  it("does without storage that refuses", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new DOMException("denied", "SecurityError");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("full", "QuotaExceededError");
    });
    expect(readWindowLayout()).toEqual({ mode: "float", rect: null });
    expect(() => writeWindowLayout({ mode: "float", rect: null })).not.toThrow();
  });
});
