"use client";

import { useSyncExternalStore } from "react";

import { listeners } from "@/lib/listeners";

// Whether a panel covers the page whole (`components/console-shell.tsx`): it
// takes the page's keys, the page's action and the rail's `g` sequences, so a
// draft in it is never lost to a stray key, and what names those keys — the
// shortcuts sheet, the rail's hints — leaves them out meanwhile. Module state,
// as a page's steps are (`lib/use-page-steps.ts`), since a tab shows one page.

let covered = false;
const { subscribe, emit } = listeners();

/** Marks the page covered or not, as the shell's covers come and go. */
export function setPageCovered(next: boolean) {
  if (covered === next) return;
  covered = next;
  emit();
}

/** Whether a panel covers the page, as a key press reads it. */
export function pageCovered(): boolean {
  return covered;
}

/** Whether a panel covers the page, for what names the keys. */
export function usePageCovered(): boolean {
  return useSyncExternalStore(subscribe, pageCovered, () => false);
}
