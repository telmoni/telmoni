"use client";

import { useEffect } from "react";

import { letterKeysOn } from "@/lib/use-accessibility";

// The keycap on the header's filled button is a real shortcut: one bare
// letter goes where the button goes. Only with nothing held down, nothing
// being typed — in a field, a list or a composition — and nothing open in
// front of the page, as the console's own letter keys (`keyboard-shortcuts.tsx`);
// a key a page has already answered is that page's. Off with every letter-key
// shortcut, in Accessibility (`lib/accessibility.ts`).
export function HeaderShortcuts({ signedIn }: { signedIn: boolean }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      if (e.metaKey || e.ctrlKey || e.altKey || e.repeat || !letterKeysOn()) return;
      const target = e.target instanceof HTMLElement ? e.target : null;
      if (
        target?.closest(
          'input, textarea, select, [contenteditable], [role="combobox"], [role="listbox"]',
        )
      )
        return;
      if (document.querySelector('[role="dialog"], [role="alertdialog"], [role="menu"]')) return;
      const key = e.key.toLowerCase();
      if (!signedIn && key === "s") window.location.assign("/auth/signup");
      if (signedIn && key === "c") window.location.assign("/console");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [signedIn]);
  return null;
}
