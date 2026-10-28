import { describe, expect, it, vi } from "vitest";

import { listeners } from "./listeners";

describe("listeners", () => {
  it("tells everyone subscribed, and nobody who left", () => {
    const { subscribe, emit } = listeners();
    const a = vi.fn();
    const b = vi.fn();
    const leaveA = subscribe(a);
    subscribe(b);
    emit();
    leaveA();
    emit();
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(2);
  });

  it("binds as the first subscribes and unbinds as the last leaves", () => {
    const first = vi.fn();
    const last = vi.fn();
    const { subscribe } = listeners({ first, last });
    const leaveA = subscribe(() => {});
    const leaveB = subscribe(() => {});
    expect(first).toHaveBeenCalledTimes(1);
    leaveA();
    expect(last).not.toHaveBeenCalled();
    leaveB();
    expect(last).toHaveBeenCalledTimes(1);
    subscribe(() => {});
    expect(first).toHaveBeenCalledTimes(2);
  });

  it("leaves one subscriber in place when another leaves twice", () => {
    const { subscribe, emit } = listeners();
    const kept = vi.fn();
    const leave = subscribe(() => {});
    subscribe(kept);
    leave();
    leave();
    emit();
    expect(kept).toHaveBeenCalledTimes(1);
  });
});
