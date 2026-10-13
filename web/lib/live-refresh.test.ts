import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LIVE_HOLD_MS, liveRefresher } from "./live-refresh";

describe("liveRefresher", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-13T12:00:00Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("draws the first nudge after a quiet spell at once", () => {
    const refresh = vi.fn();
    const live = liveRefresher(refresh);
    live.nudge();
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("holds the nudges within the hold and draws them once, when it ends", () => {
    const refresh = vi.fn();
    const live = liveRefresher(refresh);
    live.nudge();
    vi.advanceTimersByTime(5_000);
    live.nudge();
    vi.advanceTimersByTime(5_000);
    live.nudge();
    live.nudge();
    expect(refresh).toHaveBeenCalledTimes(1);

    vi.advanceTimersByTime(LIVE_HOLD_MS - 10_000 - 1);
    expect(refresh).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(1);
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it("is quiet again once the hold has passed with nothing in it", () => {
    const refresh = vi.fn();
    const live = liveRefresher(refresh);
    live.nudge();
    vi.advanceTimersByTime(LIVE_HOLD_MS);
    live.nudge();
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it("costs a flood at most two refreshes a hold", () => {
    const refresh = vi.fn();
    const live = liveRefresher(refresh);
    for (let second = 0; second < 120; second += 1) {
      live.nudge();
      vi.advanceTimersByTime(1_000);
    }
    // Two minutes of exports, one a second: the first at once, then one at
    // the end of each hold.
    expect(refresh.mock.calls.length).toBeLessThanOrEqual(1 + 120_000 / LIVE_HOLD_MS);
    expect(refresh.mock.calls.length).toBeGreaterThanOrEqual(120_000 / LIVE_HOLD_MS);
  });

  it("draws at once when flushed, and starts the hold over from there", () => {
    const refresh = vi.fn();
    const live = liveRefresher(refresh);
    live.nudge();
    live.nudge();
    live.flush();
    expect(refresh).toHaveBeenCalledTimes(2);
    vi.advanceTimersByTime(LIVE_HOLD_MS * 2);
    expect(refresh, "the nudge that was waiting was taken by the flush").toHaveBeenCalledTimes(2);
    live.nudge();
    expect(refresh).toHaveBeenCalledTimes(3);
  });

  it("drops a refresh still waiting when disposed", () => {
    const refresh = vi.fn();
    const live = liveRefresher(refresh);
    live.nudge();
    live.nudge();
    live.dispose();
    vi.advanceTimersByTime(LIVE_HOLD_MS * 2);
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("keeps time by the clock it is given", () => {
    const refresh = vi.fn();
    let now = 1_000;
    const timers: Array<{ callback: () => void; ms: number }> = [];
    const live = liveRefresher(refresh, 100, {
      now: () => now,
      setTimeout: (callback, ms) => {
        timers.push({ callback, ms });
        return timers.length;
      },
      clearTimeout: () => {},
    });
    live.nudge();
    now += 40;
    live.nudge();
    expect(timers).toEqual([{ callback: expect.any(Function), ms: 60 }]);
    now += 60;
    timers[0]?.callback();
    expect(refresh).toHaveBeenCalledTimes(2);
  });
});
