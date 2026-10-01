/**
 * The console's corner shape, as a second theme axis beside light/dark.
 *
 * `rounded` is the shipped look; `sharp` squares every curve in the console.
 * Kept out of `next-themes` because that provider carries one value and the
 * two axes are independent — a sharp dark console and a sharp light one are
 * both reachable, and folding them into one list would need four names to say
 * what two booleans already say.
 */

export const CORNERS = ["rounded", "sharp"] as const;
export type Corners = (typeof CORNERS)[number];

export const CORNERS_DEFAULT: Corners = "rounded";

/** Read by the boot script below AND by the provider; one spelling, so the
 *  first paint and the first render cannot disagree about what is stored. */
export const CORNERS_STORAGE_KEY = "telmoni-corners";

/** The class `app/globals.css` keys the sharp overrides on. It goes on
 *  `<html>`, not `<body>`: Radix portals render into `body`, and a rule that
 *  cannot reach a portalled menu would leave every dropdown round. */
export const SHARP_CLASS = "theme-sharp";

export function isCorners(value: unknown): value is Corners {
  return CORNERS.includes(value as Corners);
}

/**
 * Runs before first paint, so a sharp console never renders rounded and snaps.
 *
 * Inline and therefore CSP-relevant: `proxy.ts` serves
 * `script-src 'self' 'nonce-…'`, so the tag that carries this MUST carry the
 * request nonce or the browser refuses it and the flash comes back.
 *
 * Built from the constants above rather than written as a literal — the script
 * and the provider reading the same key is the whole contract.
 */
export const CORNERS_BOOT_SCRIPT = [
  "try{",
  `if(localStorage.getItem(${JSON.stringify(CORNERS_STORAGE_KEY)})==="sharp")`,
  `document.documentElement.classList.add(${JSON.stringify(SHARP_CLASS)})`,
  "}catch(e){}",
].join("");
