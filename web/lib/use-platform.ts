"use client";

import { useSyncExternalStore } from "react";

// Nothing to subscribe to: the platform does not change under a running tab.
// `useSyncExternalStore` is here for its OTHER half — `getServerSnapshot` —
// which is what keeps the server and the first client render agreeing.
const noop = () => () => {};

function readIsMac(): boolean {
  return /Mac|iPhone|iPad|iPod/.test(navigator.platform ?? navigator.userAgent);
}

/**
 * Whether the modifier on this keyboard is ⌘ rather than Ctrl.
 *
 * ⚠ **The server answers `false`, and that is deliberate.** There is no
 * platform to read during SSR, so the markup says "Ctrl"; a Mac swaps it to ⌘
 * on hydration. Guessing "Mac" on the server instead would put ⌘ in front of
 * every Windows user for one frame, and it is the larger population.
 */
export function useIsMac(): boolean {
  return useSyncExternalStore(noop, readIsMac, () => false);
}
