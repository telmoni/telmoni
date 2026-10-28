"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

import { inFront, pressedKey, typedInto } from "@/lib/keys";
import { letterKeysOn } from "@/lib/use-accessibility";

// The keycap on the header's filled button is a real shortcut: one bare
// letter goes where the button goes. Only with nothing held down, nothing
// being typed — in a field, a list or a composition — and nothing open in
// front of the page, as the console's own letter keys (`keyboard-shortcuts.tsx`);
// a key a page has already answered is that page's. Off with every letter-key
// shortcut, in Accessibility (`lib/accessibility.ts`).
export function HeaderShortcuts({ signedIn }: { signedIn: boolean }) {
  const router = useRouter();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || typedInto(e) || inFront()) return;
      if (e.metaKey || e.ctrlKey || e.altKey || e.repeat || !letterKeysOn()) return;
      const key = pressedKey(e).toLowerCase();
      // `/auth/signup` is a Route Handler, which only a full load reaches;
      // `/console` is a page, which the router reaches as the button does.
      // eslint-disable-next-line @next/next/no-location-assign-relative-destination
      if (!signedIn && key === "s") window.location.assign("/auth/signup");
      if (signedIn && key === "c") router.push("/console");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [signedIn, router]);
  return null;
}
