// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import {
  ACCESSIBILITY_BOOT_SCRIPT,
  ALLOW_MOTION_CLASS,
  CONTRAST_STORAGE_KEY,
  FOCUS_RINGS_CLASS,
  LETTER_KEYS_OFF_CLASS,
  LETTER_KEYS_STORAGE_KEY,
  MORE_CONTRAST_CLASS,
  MOTION_STORAGE_KEY,
  REDUCE_MOTION_CLASS,
  STANDARD_CONTRAST_CLASS,
} from "@/lib/accessibility";
import {
  bootAccessibility,
  letterKeysOn,
  setContrast,
  setFocusRings,
  setLetterKeys,
  setMotion,
  useContrast,
  useFocusRings,
  useLetterKeys,
  useMotion,
} from "@/lib/use-accessibility";

const CLASSES = [
  REDUCE_MOTION_CLASS,
  ALLOW_MOTION_CLASS,
  MORE_CONTRAST_CLASS,
  STANDARD_CONTRAST_CLASS,
  LETTER_KEYS_OFF_CLASS,
  FOCUS_RINGS_CLASS,
];

afterEach(() => {
  document.documentElement.classList.remove(...CLASSES);
  window.localStorage.clear();
});

/** What the `<head>` script does on a cold load, run against jsdom. */
function boot() {
  new Function(ACCESSIBILITY_BOOT_SCRIPT)();
}

const has = (c: string) => document.documentElement.classList.contains(c);

describe("a first visit", () => {
  it("follows the system, keeps letter keys on and draws no focus", () => {
    boot();
    for (const c of CLASSES) expect(has(c), c).toBe(false);
    expect(renderHook(() => useMotion()).result.current).toBe("system");
    expect(renderHook(() => useContrast()).result.current).toBe("system");
    expect(renderHook(() => useLetterKeys()).result.current).toBe(true);
    expect(renderHook(() => useFocusRings()).result.current).toBe(false);
  });
});

// ⚠ The boot script is a STRING, so a typo in it compiles and ships. Running
// the real constant after each setter is what proves the key it reads is the
// key the setter writes.
describe("every choice survives leaving and coming back", () => {
  it.each([
    ["reduce", REDUCE_MOTION_CLASS],
    ["allow", ALLOW_MOTION_CLASS],
  ] as const)("motion %s", (choice, cls) => {
    act(() => setMotion(choice));
    document.documentElement.classList.remove(cls);
    boot();
    expect(has(cls)).toBe(true);
    expect(renderHook(() => useMotion()).result.current).toBe(choice);
  });

  it.each([
    ["more", MORE_CONTRAST_CLASS],
    ["standard", STANDARD_CONTRAST_CLASS],
  ] as const)("contrast %s", (choice, cls) => {
    act(() => setContrast(choice));
    document.documentElement.classList.remove(cls);
    boot();
    expect(has(cls)).toBe(true);
    expect(renderHook(() => useContrast()).result.current).toBe(choice);
  });

  it("letter keys off", () => {
    act(() => setLetterKeys(false));
    document.documentElement.classList.remove(LETTER_KEYS_OFF_CLASS);
    boot();
    expect(has(LETTER_KEYS_OFF_CLASS)).toBe(true);
    expect(letterKeysOn()).toBe(false);
  });

  it("focus shown", () => {
    act(() => setFocusRings(true));
    document.documentElement.classList.remove(FOCUS_RINGS_CLASS);
    boot();
    expect(has(FOCUS_RINGS_CLASS)).toBe(true);
    expect(renderHook(() => useFocusRings()).result.current).toBe(true);
  });
});

describe("choosing System again", () => {
  it("clears the class and forgets the stored choice", () => {
    act(() => setMotion("reduce"));
    act(() => setContrast("more"));
    act(() => setMotion("system"));
    act(() => setContrast("system"));
    expect(has(REDUCE_MOTION_CLASS)).toBe(false);
    expect(has(MORE_CONTRAST_CLASS)).toBe(false);
    expect(window.localStorage.getItem(MOTION_STORAGE_KEY)).toBeNull();
    expect(window.localStorage.getItem(CONTRAST_STORAGE_KEY)).toBeNull();
  });

  it("turns letter keys back on by forgetting the off switch", () => {
    act(() => setLetterKeys(false));
    act(() => setLetterKeys(true));
    expect(letterKeysOn()).toBe(true);
    expect(window.localStorage.getItem(LETTER_KEYS_STORAGE_KEY)).toBeNull();
  });
});

// Another tab's choice lands here as well, or two windows of one console
// disagree until one reloads.
describe("another tab's choice", () => {
  it("applies here and re-renders what reads it", () => {
    const { result } = renderHook(() => useLetterKeys());
    act(() => {
      window.dispatchEvent(
        new StorageEvent("storage", { key: LETTER_KEYS_STORAGE_KEY, newValue: "1" }),
      );
    });
    expect(has(LETTER_KEYS_OFF_CLASS)).toBe(true);
    expect(result.current).toBe(false);
  });

  it("falls back to the system for a value this build does not know", () => {
    act(() => setMotion("reduce"));
    const { result } = renderHook(() => useMotion());
    act(() => {
      window.dispatchEvent(
        new StorageEvent("storage", { key: MOTION_STORAGE_KEY, newValue: "sideways" }),
      );
    });
    expect(result.current).toBe("system");
    expect(has(REDUCE_MOTION_CLASS)).toBe(false);
  });
});

// A 404 Next answers from a layout's `notFound()` is rendered from scratch in
// the browser, where the `<head>` script never runs: `bootAccessibility` is
// its stand-in there, and must do exactly what it does.
describe("a page the boot script never ran on", () => {
  it("restores every saved choice as the script would, and tells the hooks", () => {
    window.localStorage.setItem(MOTION_STORAGE_KEY, "reduce");
    window.localStorage.setItem(CONTRAST_STORAGE_KEY, "more");
    window.localStorage.setItem(LETTER_KEYS_STORAGE_KEY, "1");
    window.localStorage.setItem("telmoni-focus-rings", "1");
    const letters = renderHook(() => useLetterKeys());
    const motion = renderHook(() => useMotion());
    expect(letters.result.current).toBe(true);

    act(() => bootAccessibility());

    expect(letters.result.current).toBe(false);
    expect(motion.result.current).toBe("reduce");
    expect(document.documentElement.classList.contains(MORE_CONTRAST_CLASS)).toBe(true);
    expect(document.documentElement.classList.contains(FOCUS_RINGS_CLASS)).toBe(true);
  });

  it("agrees with the boot script for every combination storage may hold", () => {
    const after = (saved: Record<string, string | null>, run: () => void) => {
      document.documentElement.classList.remove(...CLASSES);
      window.localStorage.clear();
      for (const [key, value] of Object.entries(saved)) {
        if (value !== null) window.localStorage.setItem(key, value);
      }
      run();
      return [...document.documentElement.classList].filter((c) => CLASSES.includes(c)).sort();
    };
    for (const motion of ["reduce", "allow", "system", "nonsense", null]) {
      for (const contrast of ["more", "standard", "nonsense", null]) {
        for (const letters of ["1", "0", null]) {
          for (const focus of ["1", "yes", null]) {
            const saved = {
              [MOTION_STORAGE_KEY]: motion,
              [CONTRAST_STORAGE_KEY]: contrast,
              [LETTER_KEYS_STORAGE_KEY]: letters,
              "telmoni-focus-rings": focus,
            };
            expect(after(saved, bootAccessibility), JSON.stringify(saved)).toEqual(
              after(saved, boot),
            );
          }
        }
      }
    }
  });
});
