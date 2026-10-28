"use client";

import { useSyncExternalStore } from "react";

import { listeners } from "@/lib/listeners";

// What a page lets `j` and `k` step through — the trace list's peek, through
// the runs of the page; a session's page, through its traces — offered while
// it is open and taken back as it goes. The console's one key handler reads
// it (`components/keyboard-shortcuts.tsx`), so the keys pass the pending `g`
// first and keep the rules every bare key keeps; module state, as the trace
// list's preferences are, since a tab shows one page at a time.

/** One page's steps: what they are called, each way's move, which does
 *  nothing at its end of the page, and `open`, where `o` opens the one
 *  stood on. */
export interface PageSteps {
  /** What one step moves to, as the shortcuts sheet names it: "trace". */
  noun: string;
  next: () => void;
  previous: () => void;
  open?: () => void;
}

let current: PageSteps | null = null;
const { subscribe, emit } = listeners();

/** Offers `steps` to the keys; the function it returns takes them back,
 *  unless another offer has replaced them since. */
export function offerPageSteps(steps: PageSteps): () => void {
  current = steps;
  emit();
  return () => {
    if (current !== steps) return;
    current = null;
    emit();
  };
}

/** The steps on offer, as a key press reads them. */
export function pageSteps(): PageSteps | null {
  return current;
}

/** The steps on offer, for what draws them: the shortcuts sheet. */
export function usePageSteps(): PageSteps | null {
  return useSyncExternalStore(subscribe, pageSteps, () => null);
}
