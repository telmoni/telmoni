import { describe, expect, it } from "vitest";

import { boardOf, countedLine, formatSpend, lastRunLine } from "./monitors";
import { MonitorState } from "./types/enums";

const { Failed, Stalled, OverBound, Late, Running, New, Paused, Healthy } = MonitorState;

function inStates(...states: MonitorState[]) {
  return states.map((state, i) => ({ id: `m${i}`, state }));
}

describe("the board", () => {
  it("lists the monitors that need someone, then those running, and counts the rest", () => {
    const board = boardOf(
      inStates(Failed, Stalled, OverBound, Late, Running, New, Paused, Healthy, Healthy),
    );
    expect(board.needing.map((m) => m.state)).toEqual([Failed, Stalled, OverBound, Late]);
    expect(board.running.map((m) => m.state)).toEqual([Running]);
    expect(board.counted).toEqual([
      { state: New, count: 1 },
      { state: Paused, count: 1 },
      { state: Healthy, count: 2 },
    ]);
  });

  it("keeps the order it was given, which is the lane's: the worst first", () => {
    const board = boardOf(inStates(Late, Failed, Running, Running));
    expect(board.needing.map((m) => m.id)).toEqual(["m0", "m1"]);
    expect(board.running.map((m) => m.id)).toEqual(["m2", "m3"]);
  });

  it("counts in one line, a state with none left out", () => {
    expect(countedLine([{ state: Healthy, count: 12 }])).toBe("12 healthy");
    expect(countedLine([{ state: New, count: 2 }, { state: Healthy, count: 1234 }])).toBe(
      "2 new · 1,234 healthy",
    );
    expect(boardOf([]).counted).toEqual([]);
    expect(countedLine([])).toBe("");
  });
});

describe("a monitor's last run", () => {
  const bounds = { stall_after_s: null, max_steps: null, max_cost_usd: null };
  const run = {
    run_id: "5d4f5c68-0000-4000-8000-000000000001",
    started_at: "2026-10-21T03:00:00Z",
    ended_at: "2026-10-21T03:04:02Z",
    status: "ok" as const,
    steps: 12,
    cost_usd: 0.08,
  };

  it("reads as its verdict, its length, and its steps and spend against the caps", () => {
    expect(lastRunLine({ last_run: run, bounds })).toBe("OK · 4 min 2 s · 12 steps · $0.08");
    expect(
      lastRunLine({ last_run: run, bounds: { ...bounds, max_steps: 40, max_cost_usd: 2 } }),
    ).toBe("OK · 4 min 2 s · 12 / 40 steps · $0.08 / $2.00");
  });

  it("leaves out what the run did not report, and the length of a bare ping", () => {
    const bare = { ...run, ended_at: run.started_at, status: "error" as const };
    expect(lastRunLine({ last_run: { ...bare, steps: null, cost_usd: null }, bounds })).toBe(
      "Error",
    );
    expect(lastRunLine({ last_run: { ...run, steps: 1, cost_usd: null }, bounds })).toBe(
      "OK · 4 min 2 s · 1 step",
    );
  });

  it("is nothing until a run has ended", () => {
    expect(lastRunLine({ last_run: null, bounds })).toBeNull();
  });

  it("writes spend to the cent, and below a cent to the hundredth of one", () => {
    expect(formatSpend(0.08)).toBe("$0.08");
    expect(formatSpend(0.0012)).toBe("$0.0012");
    expect(formatSpend(1234.5)).toBe("$1,234.50");
    expect(formatSpend(0)).toBe("$0.00");
  });
});
