"use client";

import { useSyncExternalStore } from "react";

import {
  ALLOW_MOTION_CLASS,
  CONTRAST_STORAGE_KEY,
  FOCUS_RINGS_CLASS,
  FOCUS_RINGS_STORAGE_KEY,
  LETTER_KEYS_OFF_CLASS,
  LETTER_KEYS_STORAGE_KEY,
  MORE_CONTRAST_CLASS,
  MOTION_STORAGE_KEY,
  REDUCE_MOTION_CLASS,
  STANDARD_CONTRAST_CLASS,
  isContrastChoice,
  isMotionChoice,
  type ContrastChoice,
  type MotionChoice,
} from "@/lib/accessibility";
import { listeners } from "@/lib/listeners";

// ⚠ **The classes on `<html>` ARE the state — there is no second copy**, as
// with corners (`lib/use-corners.ts`): the boot script sets them before first
// paint, so state seeded from storage would be a duplicate that can disagree
// with the screen. Storage is only how a choice outlives the tab.
const root = () => document.documentElement.classList;

function applyMotion(choice: MotionChoice) {
  root().toggle(REDUCE_MOTION_CLASS, choice === "reduce");
  root().toggle(ALLOW_MOTION_CLASS, choice === "allow");
}

function applyContrast(choice: ContrastChoice) {
  root().toggle(MORE_CONTRAST_CLASS, choice === "more");
  root().toggle(STANDARD_CONTRAST_CLASS, choice === "standard");
}

// Another tab's choice lands here too, as `next-themes` syncs light and dark:
// two windows of one console must not disagree until one reloads. A value
// this build does not know falls back to following the system.
function onStorage(event: StorageEvent) {
  if (event.key === MOTION_STORAGE_KEY) {
    applyMotion(isMotionChoice(event.newValue) ? event.newValue : "system");
  } else if (event.key === CONTRAST_STORAGE_KEY) {
    applyContrast(isContrastChoice(event.newValue) ? event.newValue : "system");
  } else if (event.key === LETTER_KEYS_STORAGE_KEY) {
    root().toggle(LETTER_KEYS_OFF_CLASS, event.newValue === "1");
  } else if (event.key === FOCUS_RINGS_STORAGE_KEY) {
    root().toggle(FOCUS_RINGS_CLASS, event.newValue === "1");
  } else {
    return;
  }
  emit();
}

// Bound only while something is reading, so a page with no setting on it
// keeps no listener alive.
const { subscribe, emit } = listeners({
  first: () => window.addEventListener("storage", onStorage),
  last: () => window.removeEventListener("storage", onStorage),
});

// Storage disabled or full: the class is already on, so the choice holds for
// this tab and is forgotten on the next load.
function store(key: string, value: string | null) {
  try {
    if (value === null) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch {}
}

/** The classes as storage has them, on a page the boot script never ran on
 *  — a 404 the browser renders from Next's error shell, where no inline
 *  script runs (`components/settings-boot.tsx`); the script's own rules, so
 *  the two agree. On a page it ran on, nothing changes and nothing is told. */
export function bootAccessibility() {
  const read = (key: string) => {
    try {
      return window.localStorage.getItem(key);
    } catch {
      return null;
    }
  };
  const before = root().value;
  const motion = read(MOTION_STORAGE_KEY);
  applyMotion(motion === "reduce" || motion === "allow" ? motion : "system");
  const contrast = read(CONTRAST_STORAGE_KEY);
  applyContrast(contrast === "more" || contrast === "standard" ? contrast : "system");
  root().toggle(LETTER_KEYS_OFF_CLASS, read(LETTER_KEYS_STORAGE_KEY) === "1");
  root().toggle(FOCUS_RINGS_CLASS, read(FOCUS_RINGS_STORAGE_KEY) === "1");
  if (root().value !== before) emit();
}

export function setMotion(choice: MotionChoice) {
  applyMotion(choice);
  store(MOTION_STORAGE_KEY, choice === "system" ? null : choice);
  emit();
}

export function setContrast(choice: ContrastChoice) {
  applyContrast(choice);
  store(CONTRAST_STORAGE_KEY, choice === "system" ? null : choice);
  emit();
}

export function setLetterKeys(on: boolean) {
  root().toggle(LETTER_KEYS_OFF_CLASS, !on);
  store(LETTER_KEYS_STORAGE_KEY, on ? null : "1");
  emit();
}

export function setFocusRings(on: boolean) {
  root().toggle(FOCUS_RINGS_CLASS, on);
  store(FOCUS_RINGS_STORAGE_KEY, on ? "1" : null);
  emit();
}

/** Whether bare letter keys act as shortcuts now: read on every key press by
 *  the handlers that bind them, so a change applies at once. */
export function letterKeysOn(): boolean {
  return !root().contains(LETTER_KEYS_OFF_CLASS);
}

function focusRingsOn(): boolean {
  return root().contains(FOCUS_RINGS_CLASS);
}

function motionSnapshot(): MotionChoice {
  if (root().contains(REDUCE_MOTION_CLASS)) return "reduce";
  if (root().contains(ALLOW_MOTION_CLASS)) return "allow";
  return "system";
}

function contrastSnapshot(): ContrastChoice {
  if (root().contains(MORE_CONTRAST_CLASS)) return "more";
  if (root().contains(STANDARD_CONTRAST_CLASS)) return "standard";
  return "system";
}

// Hydration renders the defaults, then React swaps to the client snapshot,
// which keeps the server's markup and the boot script from disagreeing.
export function useMotion(): MotionChoice {
  return useSyncExternalStore(subscribe, motionSnapshot, () => "system");
}

export function useContrast(): ContrastChoice {
  return useSyncExternalStore(subscribe, contrastSnapshot, () => "system");
}

export function useLetterKeys(): boolean {
  return useSyncExternalStore(subscribe, letterKeysOn, () => true);
}

export function useFocusRings(): boolean {
  return useSyncExternalStore(subscribe, focusRingsOn, () => false);
}

// What the device itself asks for, so the page can say what "System" means
// here and now. A browser without `matchMedia` asks for nothing.
function mediaQuery(query: string) {
  const list = () =>
    typeof window.matchMedia === "function" ? window.matchMedia(query) : null;
  return {
    subscribe(onChange: () => void) {
      const media = list();
      media?.addEventListener("change", onChange);
      return () => media?.removeEventListener("change", onChange);
    },
    snapshot: () => list()?.matches ?? false,
  };
}

const reducedMotionQuery = mediaQuery("(prefers-reduced-motion: reduce)");
const moreContrastQuery = mediaQuery("(prefers-contrast: more)");

export function useSystemReducesMotion(): boolean {
  return useSyncExternalStore(reducedMotionQuery.subscribe, reducedMotionQuery.snapshot, () => false);
}

export function useSystemWantsMoreContrast(): boolean {
  return useSyncExternalStore(moreContrastQuery.subscribe, moreContrastQuery.snapshot, () => false);
}
