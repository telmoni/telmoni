"use client";

import { useEffect } from "react";

// The keycap on the header's filled button is a real shortcut: one bare
// letter, nothing held down and no field focused, goes where the button
// goes. `s` and `c` are letters the splash's secret word does not use, so
// typing it never leaves the page.
export function HeaderShortcuts({ signedIn }: { signedIn: boolean }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || e.repeat) return;
      const target = e.target instanceof HTMLElement ? e.target : null;
      if (target?.closest("input, textarea, select, [contenteditable]")) return;
      const key = e.key.toLowerCase();
      if (!signedIn && key === "s") window.location.assign("/auth/signup");
      if (signedIn && key === "c") window.location.assign("/console");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [signedIn]);
  return null;
}
