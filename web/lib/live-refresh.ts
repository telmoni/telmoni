/** How long a page holds after it was drawn for a nudge before it is drawn
 *  again for the next one: a flood of exports costs at most two refreshes in
 *  it, the first and the last, and a lone run is drawn the moment it lands.
 *  Thirty seconds unless `LIVE_REFRESH_HOLD_SECS` says otherwise
 *  (`lib/env.ts`), so an open console costs its server little however hard
 *  its project is written to. */
export const LIVE_HOLD_SECS = 30;
export const LIVE_HOLD_MS = LIVE_HOLD_SECS * 1_000;

/** The clock the refresher keeps time by; the browser's unless a test says.
 *  `Handle` is what its `setTimeout` hands back. */
export interface LiveClock<Handle> {
  now: () => number;
  setTimeout: (callback: () => void, ms: number) => Handle;
  clearTimeout: (handle: Handle) => void;
}

const BROWSER_CLOCK: LiveClock<ReturnType<typeof setTimeout>> = {
  now: () => Date.now(),
  setTimeout: (callback, ms) => setTimeout(callback, ms),
  clearTimeout: (handle) => clearTimeout(handle),
};

export interface LiveRefresher {
  /** A nudge: drawn now after a quiet spell, else once when the hold ends. */
  nudge: () => void;
  /** Drawn now, whatever the hold: a tab coming back into view. */
  flush: () => void;
  /** Drops a draw still waiting. */
  dispose: () => void;
}

/** Turns nudges into refreshes. The first after a quiet spell refreshes at
 *  once; the ones within the hold are drawn once, when it ends. By the
 *  browser's clock, unless a test hands in its own. */
export function liveRefresher(refresh: () => void, holdMs?: number): LiveRefresher;
export function liveRefresher<Handle>(
  refresh: () => void,
  holdMs: number,
  clock: LiveClock<Handle>,
): LiveRefresher;
export function liveRefresher<Handle>(
  refresh: () => void,
  holdMs: number = LIVE_HOLD_MS,
  clock?: LiveClock<Handle>,
): LiveRefresher {
  return clock ? refresherOn(refresh, holdMs, clock) : refresherOn(refresh, holdMs, BROWSER_CLOCK);
}

function refresherOn<Handle>(
  refresh: () => void,
  holdMs: number,
  clock: LiveClock<Handle>,
): LiveRefresher {
  let drawnAt: number | null = null;
  let waiting: Handle | null = null;

  const draw = () => {
    drawnAt = clock.now();
    refresh();
  };

  const dispose = () => {
    if (waiting !== null) {
      clock.clearTimeout(waiting);
      waiting = null;
    }
  };

  return {
    nudge() {
      if (waiting !== null) return;
      const since = drawnAt === null ? Infinity : clock.now() - drawnAt;
      if (since >= holdMs) {
        draw();
        return;
      }
      waiting = clock.setTimeout(() => {
        waiting = null;
        draw();
      }, holdMs - since);
    },
    flush() {
      dispose();
      draw();
    },
    dispose,
  };
}
