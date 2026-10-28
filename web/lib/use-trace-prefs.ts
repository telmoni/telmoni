"use client";

import { useSyncExternalStore } from "react";

import { listeners } from "@/lib/listeners";
import {
  DEFAULT_PREFS,
  TRACE_PREFS_KEY,
  readPrefs,
  type TracePrefs,
} from "@/lib/trace-prefs";

// ⚠ **Storage IS the state — there is no second copy.** The toolbar, which
// sets the columns, and the table, which shows them, read the same key
// through `useSyncExternalStore`, so they cannot disagree and need no
// provider between them. Another tab's choice lands here too, on the
// `storage` event, as the corners and the theme do.

/** The last value read, so a snapshot is the same object until storage
 *  changes: `useSyncExternalStore` re-renders on every new one. */
let cached: { stored: string | null; prefs: TracePrefs } | null = null;

/** What was set this session where storage is off or full, so the choice
 *  holds until the page is left. */
let unstored: TracePrefs | null = null;

function onStorage(event: StorageEvent) {
  if (event.key === TRACE_PREFS_KEY) emit();
}

const { subscribe, emit } = listeners({
  first: () => window.addEventListener("storage", onStorage),
  last: () => window.removeEventListener("storage", onStorage),
});

function getSnapshot(): TracePrefs {
  if (unstored) return unstored;
  let stored: string | null = null;
  try {
    stored = window.localStorage.getItem(TRACE_PREFS_KEY);
  } catch {
    return DEFAULT_PREFS;
  }
  if (cached?.stored === stored) return cached.prefs;
  cached = { stored, prefs: readPrefs(stored) };
  return cached.prefs;
}

// Hydration renders the defaults, then React swaps to what the browser kept,
// so the server's markup and the first client render agree.
function getServerSnapshot(): TracePrefs {
  return DEFAULT_PREFS;
}

export function setTracePrefs(patch: Partial<TracePrefs>) {
  const next = { ...getSnapshot(), ...patch };
  try {
    window.localStorage.setItem(TRACE_PREFS_KEY, JSON.stringify(next));
    unstored = null;
  } catch {
    unstored = next;
  }
  emit();
}

export function useTracePrefs(): TracePrefs {
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
}
