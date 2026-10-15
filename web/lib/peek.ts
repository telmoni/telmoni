import { PEEK_PARAM } from "@/lib/traces";

// The trace list's peek and the browser's history (`components/traces/peek.tsx`):
// the run it holds is the list's `?peek=`, moved with `pushState` and
// `replaceState`, which Next's router follows without reading the list again
// (`linking-and-navigating.md`, Native History API). Opened from a shut peek
// it is an entry of its own, so Back shuts it; stepped or switched while open
// it is the same entry, so Back still shuts it in one press; and shut from
// the panel it goes back over that entry rather than leave one more that
// shows the same list.

/** Marks a history entry the peek pushed, so shutting the peek may go back
 *  over it. */
export const PEEKED = "telmoniPeek";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** The run a `?peek=` value names, or `null` for a value that names none. */
export function peekedRun(value: string | null): string | null {
  return value && UUID.test(value) ? value : null;
}

/** The page's own URL with `run` peeked, or with the peek shut. */
export function peekUrl(run: string | null): string {
  const params = new URLSearchParams(window.location.search);
  if (run) params.set(PEEK_PARAM, run);
  else params.delete(PEEK_PARAM);
  const query = params.toString();
  return `${window.location.pathname}${query ? `?${query}` : ""}`;
}

/** Whether the history entry in front carries the peek's mark. */
export function pushedHere(): boolean {
  return (window.history.state as Record<string, unknown> | null)?.[PEEKED] === true;
}

/** Opens `run` in the peek: pushed from a shut peek, put in place of the
 *  one open otherwise. */
export function openPeek(run: string): void {
  const url = peekUrl(run);
  if (new URLSearchParams(window.location.search).has(PEEK_PARAM)) {
    window.history.replaceState({ [PEEKED]: pushedHere() }, "", url);
  } else {
    window.history.pushState({ [PEEKED]: true }, "", url);
  }
}

/** Shuts the peek: back over the entry it pushed, or, where it was opened
 *  from a link or the list has moved on since, the URL rewritten without it.
 *  `pushed` is the panel's own memory of the entry, since the mark itself
 *  does not always last (`followEntry`). */
export function closePeek(pushed: boolean): void {
  if (pushed) window.history.back();
  else window.history.replaceState(null, "", peekUrl(null));
}

/** The page's address as an entry is told by. */
function here(): string {
  return `${window.location.pathname}${window.location.search}`;
}

/** The address of the entry the peek pushed, if the one in front is it, or
 *  `null`. */
export function pushedEntry(): string | null {
  return pushedHere() ? here() : null;
}

/** The entry the peek pushed, followed through a draw: `entry` its address
 *  as last known. Next writes its own state over an entry on a refresh or
 *  an action, keeping a page's own only on Back and Forward (`HistoryUpdater`
 *  in `next/dist/client/components/app-router.js`), so at the same address
 *  the mark is written back, with Next's state in it, which its router
 *  passes straight through. At another address it is a new entry — the list
 *  moved to another window with the peek kept — and not the peek's to go
 *  back over. Still marked, it is the peek's, stepped to another run in
 *  place. */
export function followEntry(entry: string | null): string | null {
  if (entry === null) return null;
  if (pushedHere()) return here();
  if (here() !== entry) return null;
  window.history.replaceState({ ...(window.history.state ?? {}), [PEEKED]: true }, "");
  return entry;
}

/** The narrowest the panel is dragged to: the spans' pane at its least and
 *  the span beside it still readable. */
export const PEEK_WIDTH_MIN = 480;

/** The panel's width as its left edge is dragged from `startX` to `x`: wider
 *  leftwards, held between its least and the content area it stands in. */
export function draggedWidth(start: number, startX: number, x: number, area: number): number {
  return Math.round(Math.min(Math.max(start + startX - x, Math.min(PEEK_WIDTH_MIN, area)), area));
}

/** What a read of a trace's spans came to. */
export type TraceRead<T> =
  | { kind: "ok"; trace: T }
  | { kind: "missing" }
  | { kind: "forbidden" }
  | { kind: "unavailable" }
  | { kind: "signed-out" };

/** The last read kept: which URL, at which version, and what it came to. */
export type KeptRead<T> = { url: string; version: number; read: TraceRead<T> };

/** The read a new answer leaves: the answer, except where the same trace was
 *  read before and read well, and is now read again for its spans landing
 *  and fails — the trace already on show stands rather than an error in its
 *  place. A retry the person asked for after a failure takes the new answer,
 *  whatever it is. */
export function keepRead<T>(
  last: KeptRead<T> | null,
  next: KeptRead<T>,
): KeptRead<T> {
  if (last && last.url === next.url && last.read.kind === "ok" && next.read.kind !== "ok") {
    return { ...last, version: next.version };
  }
  return next;
}

/** What the panel shows of the last read while `url` at `version` is the
 *  one asked for: that read once it is the one asked for; another run's
 *  trace, marked stale, while the next run is on its way, so stepping does
 *  not blank the panel between two traces; the same run read again stands
 *  until the new answer comes, unless it was a failure, which says it is
 *  trying again. `null` while there is nothing to show. */
export function shownRead<T>(
  last: KeptRead<T> | null,
  url: string,
  version: number,
): { read: TraceRead<T> | null; stale: boolean } {
  if (!last) return { read: null, stale: false };
  if (last.url !== url) {
    return last.read.kind === "ok" ? { read: last.read, stale: true } : { read: null, stale: false };
  }
  if (last.version !== version && last.read.kind !== "ok") return { read: null, stale: false };
  return { read: last.read, stale: false };
}
