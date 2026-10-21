import type { Monitor } from "@/lib/server/entities/monitors";
import { STATUS_LABEL, formatCount, formatDuration } from "@/lib/traces";
import { MonitorState } from "@/lib/types/enums";

/** The word the console gives each state. */
export const STATE_LABEL: Record<MonitorState, string> = {
  [MonitorState.Failed]: "Failed",
  [MonitorState.Stalled]: "Stalled",
  [MonitorState.OverBound]: "Over a bound",
  [MonitorState.Late]: "Late",
  [MonitorState.Running]: "Running",
  [MonitorState.New]: "New",
  [MonitorState.Paused]: "Paused",
  [MonitorState.Healthy]: "Healthy",
};

/** Every state, the worst first: the order the board and the Monitors page
 *  sort in, held to the contract's by `contract.test.ts`. */
export const MONITOR_STATES: readonly MonitorState[] = Object.values(MonitorState);

/** The states that need someone: a run failed, gone quiet or past a bound,
 *  or a run expected that has not come. */
export const NEEDS_SOMEONE: readonly MonitorState[] = [
  MonitorState.Failed,
  MonitorState.Stalled,
  MonitorState.OverBound,
  MonitorState.Late,
];

export type Counted = { state: MonitorState; count: number };

export type Board<M extends { state: MonitorState }> = {
  /** The monitors that need someone, in the order given: the worst first. */
  needing: M[];
  /** Those with a run going. */
  running: M[];
  /** Every other monitor, counted by state in the contract's order; a state
   *  with none is left out. */
  counted: Counted[];
};

/** The board of `monitors`, in the order the lane answers them: the worst
 *  state first, then by name. */
export function boardOf<M extends { state: MonitorState }>(monitors: readonly M[]): Board<M> {
  const needing = monitors.filter((m) => NEEDS_SOMEONE.includes(m.state));
  const running = monitors.filter((m) => m.state === MonitorState.Running);
  const counted = MONITOR_STATES.filter(
    (state) => !NEEDS_SOMEONE.includes(state) && state !== MonitorState.Running,
  )
    .map((state) => ({ state, count: monitors.filter((m) => m.state === state).length }))
    .filter(({ count }) => count > 0);
  return { needing, running, counted };
}

/** The rest of the board in one line: `12 healthy`, `2 new · 12 healthy`. */
export function countedLine(counted: readonly Counted[]): string {
  return counted
    .map(({ state, count }) => `${formatCount(count)} ${STATE_LABEL[state].toLowerCase()}`)
    .join(" · ");
}

// Dollars to the cent, and below a cent to the hundredth of one, since a
// run's spend is often a fraction of a cent; in the console's own language,
// as `formatCount` is, so the server and the browser draw the same text.
const SPEND = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 4,
});

/** Spend in US dollars: `$0.08`, `$0.0012`, `$1,234.50`. */
export function formatSpend(usd: number): string {
  return SPEND.format(usd);
}

/** A monitor's last run in a line, after its time: how it ended, how long it
 *  took where it had a start, and its steps and spend where it reported any,
 *  each against the monitor's cap where one is set — `OK · 4 min 2 s · 12 /
 *  40 steps · $0.08 / $2.00`; `null` where no run has ended. */
export function lastRunLine(monitor: Pick<Monitor, "last_run" | "bounds">): string | null {
  const run = monitor.last_run;
  if (!run) return null;
  const parts = [STATUS_LABEL[run.status]];
  const ms = Date.parse(run.ended_at) - Date.parse(run.started_at);
  // A run of no duration is a bare ping, whose length nothing knows.
  if (Number.isFinite(ms) && ms > 0) parts.push(formatDuration(ms));
  if (run.steps !== null) {
    const cap = monitor.bounds.max_steps;
    const steps = cap === null ? formatCount(run.steps) : `${formatCount(run.steps)} / ${formatCount(cap)}`;
    parts.push(`${steps} ${run.steps === 1 && cap === null ? "step" : "steps"}`);
  }
  if (run.cost_usd !== null) {
    const cap = monitor.bounds.max_cost_usd;
    parts.push(
      cap === null
        ? formatSpend(run.cost_usd)
        : `${formatSpend(run.cost_usd)} / ${formatSpend(cap)}`,
    );
  }
  return parts.join(" · ");
}
