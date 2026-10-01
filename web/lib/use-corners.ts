"use client";

import { useSyncExternalStore } from "react";

import {
  CORNERS_DEFAULT,
  CORNERS_STORAGE_KEY,
  SHARP_CLASS,
  isCorners,
  type Corners,
} from "@/lib/corners";

// ⚠ **The class on `<html>` IS the state — there is no second copy.** The boot
// script sets it before first paint, so a React state seeded from storage
// would be a duplicate that can disagree with what is on screen. Reading the
// DOM through `useSyncExternalStore` leaves one source of truth and needs no
// provider, no context and no effect.
let listeners: Array<() => void> = [];

function emit() {
  for (const l of listeners) l();
}

// ⚠ **Another tab's choice has to land here too.** `next-themes` syncs light
// and dark across tabs on this same event, and a corner switch that did not
// would leave two windows of the same console disagreeing until one reloaded.
// A value this build does not know (an older spelling, or the key cleared)
// falls back to the default rather than being applied blind.
function onStorage(event: StorageEvent) {
  if (event.key !== CORNERS_STORAGE_KEY) return;
  const next = isCorners(event.newValue) ? event.newValue : CORNERS_DEFAULT;
  document.documentElement.classList.toggle(SHARP_CLASS, next === "sharp");
  emit();
}

// Bound only while something is reading, so a page with no switch on it does
// not keep a listener alive.
function subscribe(onChange: () => void) {
  if (listeners.length === 0) {
    window.addEventListener("storage", onStorage);
  }
  listeners = [...listeners, onChange];
  return () => {
    listeners = listeners.filter((l) => l !== onChange);
    if (listeners.length === 0) {
      window.removeEventListener("storage", onStorage);
    }
  };
}

function getSnapshot(): Corners {
  return document.documentElement.classList.contains(SHARP_CLASS)
    ? "sharp"
    : "rounded";
}

// Hydration renders this, then React swaps to the client snapshot — which is
// what keeps the server markup and the boot script from disagreeing.
function getServerSnapshot(): Corners {
  return CORNERS_DEFAULT;
}

export function setCorners(next: Corners) {
  document.documentElement.classList.toggle(SHARP_CLASS, next === "sharp");
  try {
    window.localStorage.setItem(CORNERS_STORAGE_KEY, next);
  } catch {
    // Storage disabled or full. The class is already on, so the choice holds
    // for this tab and is forgotten on the next load.
  }
  emit();
}

export function useCorners(): Corners {
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
}
