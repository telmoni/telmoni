/**
 * The person's accessibility settings, kept in this browser beside the theme
 * and corners: whether bare letter keys act as shortcuts, whether keyboard
 * focus is drawn, how much the interface moves, and how much contrast it
 * draws. Each is a class on
 * `<html>`, as corners are (`lib/corners.ts`): `globals.css` keys on it, and a
 * boot script sets it before first paint, so the first frame is already the
 * chosen one. Motion and contrast follow the system's own setting
 * (`prefers-reduced-motion`, `prefers-contrast`) until the person chooses
 * otherwise; no class of a pair means "follow the system".
 *
 * ⚠ **Letter-key shortcuts are WCAG 2.1's success criterion 2.1.4, at level
 * A.** A shortcut that is one bare key fires when a voice-control user
 * dictates a word or a screen reader passes keys through, so they must be
 * possible to turn off; ⌘ and Ctrl chords are outside the rule and stay on.
 */

export const MOTION_CHOICES = ["system", "reduce", "allow"] as const;
export type MotionChoice = (typeof MOTION_CHOICES)[number];

export const CONTRAST_CHOICES = ["system", "more", "standard"] as const;
export type ContrastChoice = (typeof CONTRAST_CHOICES)[number];

/** One spelling each, read by the boot script below AND by the setters, so
 *  the first paint and the first render cannot disagree. */
export const MOTION_STORAGE_KEY = "telmoni-motion";
export const CONTRAST_STORAGE_KEY = "telmoni-contrast";
/** "1" when letter-key shortcuts are off; absent when they are on. */
export const LETTER_KEYS_STORAGE_KEY = "telmoni-shortcuts-disabled";
/** "1" when keyboard focus is drawn; absent when it is not, the default. */
export const FOCUS_RINGS_STORAGE_KEY = "telmoni-focus-rings";

/** The classes `globals.css` keys on, on `<html>` (Radix portals render into
 *  `body`, so a class lower down could not reach a portalled menu). */
export const REDUCE_MOTION_CLASS = "a11y-reduce-motion";
export const ALLOW_MOTION_CLASS = "a11y-allow-motion";
export const MORE_CONTRAST_CLASS = "a11y-more-contrast";
export const STANDARD_CONTRAST_CLASS = "a11y-standard-contrast";
/** On the root while letter keys are off. Every keycap that advertises a bare
 *  key carries `data-letter-key`, and hides under this class (`globals.css`)
 *  rather than promising a key that does nothing. */
export const LETTER_KEYS_OFF_CLASS = "a11y-no-letter-keys";
/** ⚠ The console draws no focus indicator by default (`globals.css`, the
 *  unlayered `*:focus` rule), short of WCAG 2.1's 2.4.7 at level AA; this
 *  class is the way back to one, a person's own choice. */
export const FOCUS_RINGS_CLASS = "a11y-focus-rings";

export function isMotionChoice(value: unknown): value is MotionChoice {
  return MOTION_CHOICES.includes(value as MotionChoice);
}

export function isContrastChoice(value: unknown): value is ContrastChoice {
  return CONTRAST_CHOICES.includes(value as ContrastChoice);
}

/**
 * Runs before first paint, in the same inline script as the corners'
 * (`app/layout.tsx`), which carries the request nonce `proxy.ts`'s CSP asks
 * for. Built from the constants above rather than written as a literal: the
 * script and the setters reading the same keys is the whole contract.
 */
export const ACCESSIBILITY_BOOT_SCRIPT = [
  "try{",
  "var s=localStorage,c=document.documentElement.classList,",
  `m=s.getItem(${JSON.stringify(MOTION_STORAGE_KEY)}),`,
  `k=s.getItem(${JSON.stringify(CONTRAST_STORAGE_KEY)});`,
  `if(m==="reduce")c.add(${JSON.stringify(REDUCE_MOTION_CLASS)});`,
  `else if(m==="allow")c.add(${JSON.stringify(ALLOW_MOTION_CLASS)});`,
  `if(k==="more")c.add(${JSON.stringify(MORE_CONTRAST_CLASS)});`,
  `else if(k==="standard")c.add(${JSON.stringify(STANDARD_CONTRAST_CLASS)});`,
  `if(s.getItem(${JSON.stringify(LETTER_KEYS_STORAGE_KEY)})==="1")`,
  `c.add(${JSON.stringify(LETTER_KEYS_OFF_CLASS)});`,
  `if(s.getItem(${JSON.stringify(FOCUS_RINGS_STORAGE_KEY)})==="1")`,
  `c.add(${JSON.stringify(FOCUS_RINGS_CLASS)});`,
  "}catch(e){}",
].join("");
